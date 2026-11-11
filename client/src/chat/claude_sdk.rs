//! 和 `claude -p --input-format stream-json --output-format stream-json` 说话（docs/CLAUDE_SDK.md）。
//!
//! 这是 Claude Agent SDK 用的同一套协议：每行一条 JSON，走子进程的 stdin / stdout。用户自己装的 `claude` 就能用，
//! 不需要 SDK 本身（SDK 也只是起这个进程）。
//!
//! - 看看发：用户消息（`{"type":"user",…}`）、控制请求（`initialize`、`interrupt`、`set_model`、`set_permission_mode`、
//!   `apply_flag_settings`、`stop_task`……），控制请求按 `request_id` 对上答复；
//! - claude 发：对话消息（`system`、`assistant`、`user`、`result`、`stream_event`……），
//!   以及要看看答复的控制请求（`can_use_tool` 审批、`hook_callback` 注册的 hook），用 [`Proc::respond`] 答。
//!
//! 一个对话模式的 Claude Code 终端一个进程，看看是它的父进程：客户端退出时它也退出（stdin 关了），
//! 下次用到时 `--resume` 接着同一段对话（对话记录照常写在 `projects/` 下）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::error::LocalError;

/// 控制请求等答复的上限（`initialize` 要加载插件、MCP，慢的机器上要几秒）
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// 进程出错时最多留下这么多行 stderr（界面上显示最后几行）
const STDERR_KEEP: usize = 40;

/// claude 发来的
#[derive(Debug)]
pub enum Incoming {
    /// 对话消息，或者要答复的控制请求（`type` 为 `control_request`）
    Message(Value),
    /// 进程退出了：退出码、最后几行 stderr
    Exited { code: Option<i32>, stderr: String },
}

type Waiter = oneshot::Sender<Result<Value, String>>;

pub struct Proc {
    stdin: tokio::sync::Mutex<tokio::process::ChildStdin>,
    waiting: Mutex<HashMap<String, Waiter>>,
    next: AtomicU64,
    alive: AtomicBool,
    /// 是看看结束的（不是自己退出、崩溃）
    killed: AtomicBool,
    pub pid: Option<u32>,
    pub started_at: std::time::Instant,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 怎么起 claude
pub struct Spawn {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub profile_env: Option<crate::model::LaunchEnv>,
    pub path: Option<std::ffi::OsString>,
}

impl Proc {
    /// 起进程，开始收发。收到的消息和退出都发到 `tx`。
    pub fn spawn(s: Spawn, tx: mpsc::UnboundedSender<Incoming>) -> Result<Arc<Self>, LocalError> {
        let mut cmd = command(&s.program);
        cmd.args(&s.args).current_dir(&s.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        if let Some(p) = &s.path {
            cmd.env("PATH", p);
        }
        // 和 tmux 里起的一样：保证 UTF-8，去掉外层 tmux、外层 Claude Code 的标记（客户端是在 Claude Code 里重启的时候
        // 会带着 CLAUDE_CODE_CHILD_SESSION，claude 就不存对话记录了）
        crate::instances::apply_env(&mut cmd);
        // 终端里的标识不传进去（hook 会把它当成 TUI 会话）
        cmd.env_remove(super::hooks::INSTANCE_ENV);
        for (k, v) in &s.env {
            cmd.env(k, v);
        }
        if let Some(e) = &s.profile_env {
            e.apply(&mut cmd);
        }
        #[cfg(unix)]
        cmd.process_group(0);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| LocalError::new("CLAUDE_SDK_START").with("detail", format!("{}: {e}", s.program.display())))?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let me = Arc::new(Proc { stdin: tokio::sync::Mutex::new(stdin), waiting: Mutex::new(HashMap::new()), next: AtomicU64::new(1), alive: AtomicBool::new(true), killed: AtomicBool::new(false), pid, started_at: std::time::Instant::now() });
        let tail: Arc<Mutex<std::collections::VecDeque<String>>> = Arc::default();
        {
            let tail = tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    tracing::debug!(target: "claude_sdk", "stderr: {l}");
                    let mut t = lock(&tail);
                    t.push_back(l);
                    while t.len() > STDERR_KEEP {
                        t.pop_front();
                    }
                }
            });
        }
        let weak = Arc::downgrade(&me);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                // 一行可能很大（整份工具结果、图片），按字节读，不限长度
                match lines.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let Ok(v) = serde_json::from_slice::<Value>(&buf) else {
                    let s = String::from_utf8_lossy(&buf);
                    if !s.trim().is_empty() {
                        tracing::debug!(target: "claude_sdk", "不是 JSON 的输出：{}", s.chars().take(300).collect::<String>());
                    }
                    continue;
                };
                if v["type"] == "control_response" {
                    if let Some(p) = weak.upgrade() {
                        p.resolve(&v["response"]);
                    }
                    continue;
                }
                if tx.send(Incoming::Message(v)).is_err() {
                    break;
                }
            }
            let code = tokio::time::timeout(Duration::from_secs(5), child.wait()).await.ok().and_then(|r| r.ok()).and_then(|s| s.code());
            if let Some(p) = weak.upgrade() {
                p.alive.store(false, Ordering::SeqCst);
                for (_, w) in lock(&p.waiting).drain() {
                    let _ = w.send(Err("claude exited".into()));
                }
            }
            // 等 stderr 读完最后几行
            tokio::time::sleep(Duration::from_millis(100)).await;
            let stderr = lock(&tail).iter().cloned().collect::<Vec<_>>().join("\n");
            let _ = tx.send(Incoming::Exited { code, stderr });
        });
        Ok(me)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }

    fn resolve(&self, r: &Value) {
        let Some(id) = r["request_id"].as_str() else { return };
        let Some(w) = lock(&self.waiting).remove(id) else { return };
        let out = if r["subtype"] == "success" { Ok(r["response"].clone()) } else { Err(r["error"].as_str().unwrap_or("error").to_string()) };
        let _ = w.send(out);
    }

    async fn write(&self, v: &Value) -> Result<(), LocalError> {
        if !self.alive() {
            return Err(LocalError::new("CHAT_NOT_RUNNING"));
        }
        let mut line = serde_json::to_vec(v).map_err(|e| LocalError::from(anyhow::anyhow!(e)))?;
        line.push(b'\n');
        let mut w = self.stdin.lock().await;
        w.write_all(&line).await.map_err(|_| LocalError::new("CHAT_NOT_RUNNING"))?;
        w.flush().await.map_err(|_| LocalError::new("CHAT_NOT_RUNNING"))
    }

    /// 控制请求，等答复（`response` 字段）
    pub async fn request(&self, subtype: &str, mut body: Value) -> Result<Value, LocalError> {
        let id = format!("ll_{}", self.next.fetch_add(1, Ordering::Relaxed));
        body["subtype"] = json!(subtype);
        let (tx, rx) = oneshot::channel();
        lock(&self.waiting).insert(id.clone(), tx);
        if let Err(e) = self.write(&json!({ "type": "control_request", "request_id": id, "request": body })).await {
            lock(&self.waiting).remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(CALL_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(LocalError::new("CLAUDE_SDK_ERROR").with("detail", e)),
            Ok(Err(_)) => Err(LocalError::new("CHAT_NOT_RUNNING")),
            Err(_) => {
                lock(&self.waiting).remove(&id);
                Err(LocalError::new("CLAUDE_SDK_TIMEOUT").with("detail", subtype.to_string()))
            }
        }
    }

    /// 答复 claude 发来的控制请求
    pub async fn respond(&self, request_id: &str, response: Value) -> Result<(), LocalError> {
        self.write(&json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": response } })).await
    }

    pub async fn respond_error(&self, request_id: &str, error: &str) -> Result<(), LocalError> {
        self.write(&json!({ "type": "control_response", "response": { "subtype": "error", "request_id": request_id, "error": error } })).await
    }

    /// 发一条用户消息（`content` 是字符串或内容块数组）。这一轮还在跑时 claude 先排着，做完这一步再并进去。
    pub async fn send_user(&self, content: Value) -> Result<(), LocalError> {
        self.send_user_typed(content, false).await
    }

    /// 同上；`ultrathink`：用户在输入框里手打了 ultrathink（`ultrathink_typed`，claude 这一轮按 xhigh 想，跟 TUI 里一样）。
    /// 只在最近一次 init 带了 `ultrathink_typed_raises: true` 时用。
    pub async fn send_user_typed(&self, content: Value, ultrathink: bool) -> Result<(), LocalError> {
        self.send_user_with(content, ultrathink, false).await
    }

    /// `now`：TUI 的“立即发送”（`priority: "now"`）——这一步一结束就停下这一轮、先回答这条，没做完的事不再接着做。
    /// 配合 `background_tasks`（先把前台命令、子代理转到后台）就跟终端里按立即发送一样：不用等命令跑完。
    /// 带 uuid：claude 只在有 uuid 时能把它当成可以抢先的那条（turn_preempted 的 preempted_by_uuid）
    pub async fn send_user_with(&self, content: Value, ultrathink: bool, now: bool) -> Result<(), LocalError> {
        let mut m = json!({ "type": "user", "message": { "role": "user", "content": content }, "parent_tool_use_id": null, "session_id": "" });
        if ultrathink {
            m["ultrathink_typed"] = json!(true);
        }
        if now {
            m["priority"] = json!("now");
            m["uuid"] = json!(uuid_v4());
        }
        self.write(&m).await
    }

    /// 结束进程（连同它起的命令：单独一个进程组）
    pub fn kill(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.killed.store(true, Ordering::SeqCst);
        let Some(pid) = self.pid else { return };
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        #[cfg(windows)]
        {
            use sysinfo::{Pid, ProcessesToUpdate, System};
            let mut sys = System::new();
            let p = Pid::from_u32(pid);
            sys.refresh_processes(ProcessesToUpdate::Some(&[p]), true);
            if let Some(proc_) = sys.process(p) {
                proc_.kill();
            }
        }
    }
}

/// Windows 上 npm 装的是 `claude.cmd`：要经 cmd 运行
pub(super) fn command(program: &std::path::Path) -> tokio::process::Command {
    let ext = program.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    if cfg!(windows) && matches!(ext.as_deref(), Some("cmd" | "bat")) {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/C").arg(program);
        return c;
    }
    tokio::process::Command::new(program)
}

/// 用哪个 `claude`：客户端 PATH 里的，没有时在用户 shell 的 PATH 里找，再没有就用常见的安装位置
/// （`~/.local/bin/claude`、Claude 桌面版自带的，见 [`crate::agents::claude_fallback`]）。
pub async fn program(shell: &str) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) { &["claude.exe", "claude.cmd", "claude.bat"] } else { &["claude"] };
    let find = |path: &std::ffi::OsStr| std::env::split_paths(path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file());
    if let Some(p) = std::env::var_os("PATH").and_then(|path| find(&path)) {
        return Some(p);
    }
    if let Some(p) = super::codex_app::user_path(shell).await.and_then(|path| find(path.as_ref())) {
        return Some(p);
    }
    crate::agents::claude_fallback().map(PathBuf::from)
}

/// 注册给 claude 的 hook（`initialize` 的 `hooks`）：事件名就是 callback id，答复时按事件名转给对话核心
/// （和终端里 `looklook hook` 转发的是同一种输入，见 `Chat::hook`）。审批不用 hook，走 `can_use_tool`。
pub const HOOK_EVENTS: &[&str] = &["SessionStart", "SessionEnd", "UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "Stop", "StopFailure", "SubagentStart", "SubagentStop", "PreCompact", "PostCompact"];

pub fn hooks_payload() -> Value {
    let mut m = serde_json::Map::new();
    for e in HOOK_EVENTS {
        // 看看答不上来（出错、退出）时照常运行，不挡住 claude
        m.insert((*e).to_string(), json!([{ "hookCallbackIds": [format!("ll:{e}")], "onFailure": "continue" }]));
    }
    Value::Object(m)
}

/// 用户消息的内容：有图片时是内容块数组（图片按 base64 内嵌），否则就是文字
pub fn user_content(text: &str, images: &[String]) -> Result<Value, LocalError> {
    use base64::Engine;
    if images.is_empty() {
        return Ok(json!(text));
    }
    let mut blocks = vec![];
    for p in images {
        let path = std::path::Path::new(p);
        let media = match path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            _ => return Err(LocalError::invalid("images", "invalid")),
        };
        let bytes = std::fs::read(path).map_err(|_| LocalError::invalid("images", "not_found"))?;
        blocks.push(json!({ "type": "image", "source": { "type": "base64", "media_type": media, "data": base64::engine::general_purpose::STANDARD.encode(bytes) } }));
    }
    if !text.is_empty() {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    Ok(Value::Array(blocks))
}

/// 在各个账号的 `projects/` 下找会话的记录文件（接着以前的对话时，还没收到 hook 之前就能显示历史）
pub fn find_transcript(config_dirs: &[PathBuf], session: &str) -> Option<PathBuf> {
    if !crate::history::valid_id(session) {
        return None;
    }
    let name = format!("{session}.jsonl");
    for dir in config_dirs {
        let Ok(rd) = std::fs::read_dir(dir.join("projects")) else { continue };
        for e in rd.flatten() {
            let p = e.path().join(&name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_cover_events() {
        let h = hooks_payload();
        assert_eq!(h["Stop"][0]["hookCallbackIds"][0], "ll:Stop");
        assert_eq!(h["PreToolUse"][0]["onFailure"], "continue");
        assert!(h.get("PermissionRequest").is_none(), "审批走 can_use_tool");
    }

    #[test]
    fn content_plain_and_images() {
        assert_eq!(user_content("hi", &[]).unwrap(), json!("hi"));
        let dir = std::env::temp_dir().join(format!("ll-sdk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("a.png");
        std::fs::write(&png, b"\x89PNG").unwrap();
        let v = user_content("look", &[png.display().to_string()]).unwrap();
        assert_eq!(v[0]["source"]["media_type"], "image/png");
        assert_eq!(v[1]["text"], "look");
        assert!(user_content("x", &[dir.join("a.txt").display().to_string()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_transcripts() {
        let dir = std::env::temp_dir().join(format!("ll-sdk-tr-{}", std::process::id()));
        let proj = dir.join("projects").join("-w-app");
        std::fs::create_dir_all(&proj).unwrap();
        let id = "0f2e1837-1c82-4ae3-9dfa-557e9928f9dc";
        std::fs::write(proj.join(format!("{id}.jsonl")), "").unwrap();
        assert_eq!(find_transcript(&[dir.clone()], id), Some(proj.join(format!("{id}.jsonl"))));
        assert_eq!(find_transcript(&[dir.clone()], "../etc"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 随机 uuid（v4 格式），给 claude 认消息用
fn uuid_v4() -> String {
    let b: [u8; 16] = rand::random();
    let h = |r: std::ops::Range<usize>| b[r].iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!("{}-{}-4{}-{:x}{}-{}", h(0..4), h(4..6), &h(6..8)[1..], 8 | (b[8] & 3), &h(8..10)[1..], h(10..16))
}
