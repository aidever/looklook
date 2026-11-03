//! 清理后台会话：会话停在后台、很久没在界面上打开过，tmux 会话和里面的程序就一直占着。
//! - 自动：设置里的“几天没打开就停掉”（默认 7 天），每 10 分钟看一次。正在忙的（AI 在回复、等你确认，普通终端有命令在跑）不停；
//!   终端里这些天有过输出的也不停（界面没打开，但在飞书里聊、AI 自己在跑），见 [`Idle::active_at`]。
//! - 手动：会话列表的“清理后台会话”，列出在后台的会话，用户勾选、确认后停掉（[`list`]、gateway 的 `/instances/idle`）。
//!
//! 停掉 = 结束 tmux 会话和里面的程序，会话本身留着，点开就重新启动；AI 工具的会话下次启动接着原来的对话
//! （见 [`crate::instances::Instances::stop_idle`]）。

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::chat::Chat;
use crate::instances::{Instances, View};
use crate::store::Store;

const EVERY: Duration = Duration::from_secs(600);
const DAY_MS: i64 = 86_400_000;

/// 一个在后台的会话
#[derive(Debug, Serialize)]
pub struct Idle {
    pub id: String,
    pub name: String,
    pub title: Option<String>,
    pub launch: String,
    pub tool: Option<&'static str>,
    pub workdir: String,
    /// 最近一次打开（毫秒）
    pub opened_at: i64,
    /// 最近一次有动静（毫秒）：界面打开过，或者终端里有输出（飞书里聊、AI 自己在跑都算）
    pub active_at: i64,
    /// 正在忙：停掉会打断它
    pub busy: bool,
    /// 普通终端在跑的程序
    pub fg: Option<String>,
}

/// 在忙：对话界面在跑、等你确认；看进程认出来在忙（AI 在回复、普通终端有命令在跑）
fn busy(chat: &Chat, v: &View) -> bool {
    let phase = chat.brief(&v.row.id).and_then(|b| b["phase"].as_str().map(String::from));
    matches!(phase.as_deref(), Some("running" | "approval" | "tui")) || v.busy == Some(true)
}

/// 在后台跑着的会话（tmux 会话还在），很久没动静的在前
pub async fn list(instances: &Instances, chat: &Chat) -> Vec<Idle> {
    let views = instances.list().await.unwrap_or_default();
    let output = instances.output_activity().await;
    let mut out: Vec<Idle> = views
        .iter()
        .filter(|v| v.running || v.task_alive == Some(true))
        .map(|v| {
            let opened_at = instances.opened_at(&v.row.id);
            Idle {
                id: v.row.id.clone(),
                name: v.row.name.clone(),
                title: v.agent_title.clone(),
                launch: v.row.launch.clone(),
                tool: v.tool,
                workdir: v.row.workdir.clone(),
                opened_at,
                active_at: output.get(&v.row.id).copied().unwrap_or(0).max(opened_at),
                busy: busy(chat, v),
                fg: v.fg.clone(),
            }
        })
        .collect();
    out.sort_by_key(|i| i.active_at);
    out
}

/// 停掉这些会话（用户在“清理后台会话”里勾选的），返回停掉了几个
pub async fn stop(instances: &Instances, ids: &[String]) -> usize {
    let views = instances.list().await.unwrap_or_default();
    let mut n = 0;
    for v in views.iter().filter(|v| ids.contains(&v.row.id) && (v.running || v.task_alive == Some(true))) {
        match instances.stop_idle(v).await {
            Ok(()) => n += 1,
            Err(e) => tracing::warn!(id = %v.row.id, error = ?e.code, "清理后台会话：停不掉"),
        }
    }
    n
}

/// 自动停掉很久没打开、也没在忙的会话
pub async fn run(store: Arc<Store>, instances: Arc<Instances>, chat: Arc<Chat>) {
    loop {
        tokio::time::sleep(EVERY).await;
        let days = crate::settings::Settings::load(&store).normalized().idle_stop_days;
        if days == 0 {
            continue;
        }
        let before = crate::util::system_ms() - i64::from(days) * DAY_MS;
        let stale: Vec<String> = list(&instances, &chat).await.into_iter().filter(|i| i.active_at < before && !i.busy).map(|i| i.id).collect();
        if !stale.is_empty() {
            let n = stop(&instances, &stale).await;
            tracing::info!(days, n, "自动停掉了很久没打开的后台会话");
        }
    }
}
