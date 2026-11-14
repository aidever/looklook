//! 对话模式的 Claude Code 走 stream-json（docs/CLAUDE_SDK.md）：看看起 `claude -p --input-format stream-json …`，
//! 不在终端里跑 TUI、不抓屏、不“打字”。终端标签里是同一个工作目录下的普通 shell。
//!
//! 和 TUI 方案共用的：对话内容照旧读 transcript（`transcript.rs`），状态机照旧是 [`Chat::hook`]——
//! hook 不再经 `looklook hook` 子命令和 HTTP 转过来，而是在 `initialize` 里注册、由 claude 通过 stdio 回调
//! （`hook_callback`），输入一模一样。新的只有这些：
//!
//! - 发消息：一条 `user` 消息（图片按 base64 内嵌）；运行中发的由 claude 排队，做完这一步再并进去；
//! - 停止：`interrupt`；模型、思考强度、权限模式：`set_model`、`apply_flag_settings`、`set_permission_mode`；
//! - 审批、提问、计划：`can_use_tool` 控制请求，挂起等界面回答，没有超时（不再有 TUI 兜底）；
//! - 正在输出的回复：`stream_event` 里的增量（`--include-partial-messages`）；
//! - 后台任务个数：`background_tasks_changed`；停止后台任务：`stop_task`。
//!
//! 进程是看看的子进程：客户端重启时它也退出，下次打开对话或发消息时 `--resume` 接着同一段。
//! 空闲、没人看、没有后台任务超过 [`IDLE_REAP`] 的也结束掉，省内存。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

use super::claude_sdk::{self, Incoming, Proc};
use super::{board, lock, models, Chat, Event, Inst, Pending, Phase, PermissionAnswer};
use crate::error::LocalError;

/// 正在输出时最多每隔这么久推一次状态
const PREVIEW_GAP: Duration = Duration::from_millis(120);
/// 正在输出的回复最多推这么多字（只留最后这段）
const PREVIEW_LIMIT: usize = 4000;
/// 空闲多久、没人看、没有后台任务就结束进程（下次用到时 `--resume`）
const IDLE_REAP: Duration = Duration::from_secs(30 * 60);
/// 自动起名（[`Chat::sdk_auto_title`]）：第一轮做完起一次，之后至少隔这么多轮、这么久再起一次（跟着对话的进展换名字）
const TITLE_TURNS: u32 = 3;
const TITLE_GAP: Duration = Duration::from_secs(10 * 60);
/// 起名最多等多久
const TITLE_WAIT: Duration = Duration::from_secs(60);
/// 起来后这么快就退出的算“起不来”（没登录、参数不认、程序坏了），界面上显示原因，不自动重起
const QUICK_EXIT: Duration = Duration::from_secs(15);
/// 额度用完后等重置自动继续：到了重置时间再多等这么久才发 continue（重置时间是整点，留点余量）
const LIMIT_SLACK_MS: i64 = 60_000;
/// 不知道什么时候重置（第三方接口多半不给）：过这么久试一次，还没重置会再失败，下次多等一些（10、20、40 分钟，之后每小时）
const LIMIT_RETRY_MS: [i64; 4] = [10 * 60_000, 20 * 60_000, 40 * 60_000, 60 * 60_000];
/// 等重置时隔这么久看一次用量：没到点但额度已经回来了（比如等着 5 小时窗口，周额度提前刷新了；官方提前重置）就提前继续
const LIMIT_POLL_MS: i64 = 10 * 60_000;

#[derive(Default)]
pub struct Hub {
    /// 同时只起一个进程（打开对话和发消息可能同时要起）
    start: tokio::sync::Mutex<()>,
}

/// 每个终端的 stream-json 状态（在 [`super::State`] 里）
#[derive(Default)]
pub struct SdkSt {
    pub(super) proc: Option<Arc<Proc>>,
    /// 装了更新的 Claude Code（这个进程还是旧版本）：空闲时结束它，下次发消息时用新版本接着（[`Chat::sdk_upgrade`]）
    pub(super) upgrade: Option<String>,
    /// 已经为这个版本重起过（起来的还是旧版本时不再一遍遍重起）
    upgraded: Option<String>,
    /// 新建时选的“接着以前的对话”：起进程时用一次（会话号、要不要复制一份）
    pub(super) resume: Option<(String, bool)>,
    /// 编辑重发：接着时只要到这条记录为止（`--resume-session-at`），起进程时用一次
    resume_at: Option<String>,
    /// 第一句话：起来后发
    pub(super) first: Option<String>,
    /// 后台任务：工具调用 id → task id（停止用）
    tasks: HashMap<String, String>,
    /// 在界面上点了停止，这一轮算“中断”
    interrupted: bool,
    /// 这一轮在跑：不自动更新（更新要重启客户端，会把这个进程一起结束）
    turn_guard: Option<crate::updater::TerminalGuard>,
    preview_at: Option<Instant>,
    /// 从什么时候起空闲（结束空闲进程用）
    idle_since: Option<Instant>,
    /// 在界面上选的模型、思考强度、权限模式：进程重起时带上
    model: Option<String>,
    effort: Option<String>,
    mode: Option<String>,
    /// 自动起名：上次起名之后做完了几轮、上次是什么时候、是不是正在起
    title_turns: u32,
    title_at: Option<Instant>,
    titling: bool,
    /// 最近一次 `rate_limit_event` 说额度用完了（`status: rejected`）：什么时候重置（Unix 秒）
    limit_reset: Option<i64>,
    /// 额度用完、在等重置后自动发 continue：什么时候发（毫秒）。TUI 里是 Claude Code 自己等（默认开着），这里看看替它等
    /// 存在数据库里（[`limit_key`]），客户端重启后接着等
    pub(super) limit_resume: Option<i64>,
    /// 到点了、正在发 continue
    limit_firing: bool,
    /// 等重置期间下次看用量的时间（毫秒）；None 不看。`limit_since` 之前取的用量不算数（那时还没用完）
    limit_poll: Option<i64>,
    limit_since: i64,
    limit_checking: bool,
    /// 这次 continue 是看用量提前发的：要是马上又撞上限额（用量里看不出来的限额，比如单个模型的周额度），这一轮就只等到点
    limit_early: bool,
    /// 不知道什么时候重置（第三方接口多半不说）：现在等的是猜的时间，界面上照实说；
    /// 连着猜了几次（每次多等一些，见 [`LIMIT_RETRY_MS`]），一轮正常做完后清零
    pub(super) limit_guess: bool,
    pub(super) limit_tries: u32,
    /// 交给终端了（毫秒，什么时候交的）：终端里的 TUI（`claude --resume`）在接着这段对话，对话页只看；
    /// 在对话页发消息时收回来（[`Chat::sdk_take_back`]）。存在数据库里（[`term_key`]），客户端重启后还认
    pub(super) term: Option<i64>,
    /// 打开终端标签时这一轮还在跑：做完后再交给终端
    term_after: bool,
    /// 对话页正在发的消息条数（起进程、送消息途中）：这时不交给终端，见 [`Chat::sdk_to_terminal`]
    sending: u32,
    /// Stop hook 的 `session_crons` 不是空的：这段对话里有定时任务（/loop、CronCreate、ScheduleWakeup），
    /// 都在 claude 进程里，进程结束就没了，所以不能当空闲结束
    pub(super) crons: bool,
    /// 快速模式：用户在这段对话里手打 `/fast` 选的（进程重起时带上）；None 时按账号设置里的 `fastMode`（TUI 的 /fast 存在那里）
    fast: Option<bool>,
    /// claude 最近报的快速模式状态（`fast_mode_state` 是 on / cooldown）：手打不带参数的 `/fast` 时按它切换
    fast_on: bool,
    /// 最近一次 init 的 `ultrathink_typed_raises`：用户手打了 ultrathink 时要在消息上说明（见 [`Chat::sdk_input`]）
    ultrathink: bool,
    /// 最近一轮 `result` 的 `modelUsage` 里 Claude Code 自己认的各模型上下文窗口（模型 → token 数）：
    /// 第三方接口没有账号的模型目录，看看自己推算不出来，以它为准
    pub(super) windows: HashMap<String, u64>,
}

/// 用户登录 shell 里的这些变量不带给 claude：这个 shell 自己的（目录、层数、终端）、看看的终端标识、
/// PATH（另外算，见 `codex_app::launch_path`）、配置目录（按会话的账号定）、Claude Code 给它起的子进程设的（客户端本身在
/// Claude Code 里跑时会带着）
const SDK_ENV_SKIP: &[&str] = &[
    "PATH", "PWD", "OLDPWD", "SHLVL", "_", "TMUX", "TMUX_PANE", "TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "COLORTERM", "CLAUDECODE", "CLAUDE_CONFIG_DIR",
    "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_SESSION_ID", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ATTENDED", "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_SSE_PORT", super::hooks::INSTANCE_ENV,
];

/// stream-json 的 claude 的环境：补上终端里起的 claude 有、看看直接起的没有的——
/// - 用户登录 shell 的环境变量（`~/.zshrc` 里的代理、Key 等）和 `SHELL`（Bash 工具按它选 shell、读它的配置）；
/// - `CLAUDE_CODE_ARTIFACT=1`：`-p` 下 Artifact（claude.ai 网页）默认关着（sdk_default_off），终端里是开着的。
///   用户自己设了（包括设成 0）就照他的；关掉 Artifact 的设置（`disableArtifact`、`CLAUDE_CODE_DISABLE_ARTIFACT`）照样管用。
fn sdk_env(shell: &str, user: Vec<(String, String)>) -> Vec<(String, String)> {
    let artifact_set = std::env::var_os("CLAUDE_CODE_ARTIFACT").is_some() || user.iter().any(|(k, _)| k == "CLAUDE_CODE_ARTIFACT");
    // 和客户端自己的一样的不用再设（本来就继承）
    let mut env: Vec<(String, String)> = user
        .into_iter()
        .filter(|(k, _)| !SDK_ENV_SKIP.contains(&k.as_str()) && !k.starts_with("LOOKLOOK_"))
        .filter(|(k, v)| std::env::var(k).ok().as_deref() != Some(v.as_str()))
        .collect();
    if !cfg!(windows) && !shell.is_empty() {
        env.push(("SHELL".into(), shell.to_string()));
    }
    if !artifact_set {
        env.push(("CLAUDE_CODE_ARTIFACT".into(), "1".into()));
    }
    env
}

/// 账号设置里开着快速模式（终端里 `/fast` 打开后存在 `settings.json` 的 `fastMode`）
fn account_fast(config_dir: &std::path::Path) -> bool {
    std::fs::read(config_dir.join("settings.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).is_some_and(|v| v["fastMode"] == true)
}

impl Chat {
    /// 这个终端的对话走 stream-json（新建的对话模式 Claude Code；旧的 TUI 会话照旧，见 `Instances::claude_sdk_mode`）
    pub(super) fn is_sdk(&self, id: &str) -> bool {
        self.store.instance(id).ok().flatten().is_some_and(|r| self.instances.claude_sdk_mode(&r))
    }

    /// 新建会话时的第一句话、接着哪段对话：记下，起进程时用
    pub async fn sdk_first(self: &Arc<Self>, id: &str, prompt: Option<String>, resume: Option<crate::instances::Resume>) {
        let inst = self.inst(id);
        {
            let mut st = lock(&inst.st);
            if let Some(r) = &resume {
                st.sdk.resume = Some((r.id.clone(), r.fork));
                // 接着的（不是复制的）这段马上能显示历史
                if !r.fork {
                    let dirs = self.claude_config_dirs();
                    if let Some(path) = claude_sdk::find_transcript(&dirs, &r.id) {
                        let b = super::Binding { session_id: r.id.clone(), transcript_path: path };
                        let _ = self.store.set(&super::binding_key(id), &b);
                        crate::history::mark_own(&self.store, &r.id);
                        st.binding = Some(b);
                    }
                }
            }
            st.sdk.first = prompt.clone();
            if prompt.is_some() {
                Self::note_prompt(&mut st, prompt.as_deref().unwrap_or(""));
            }
        }
        if let Some(p) = &prompt {
            self.record_input(id, p);
        }
        if prompt.is_some() {
            if let Err(e) = self.sdk_attach(id).await {
                tracing::warn!(id, error = ?e, "对话模式的 Claude Code 没起来，第一句话没发出去");
            }
        }
    }

    /// 各个账号的 Claude Code 配置目录（找对话记录用）
    fn claude_config_dirs(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = vec![];
        for p in crate::claude_profiles::list(&self.store) {
            let dir = crate::claude_profiles::config_paths(&self.paths.home, &p).0;
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        dirs
    }

    /// 起好（或者拿到已经在跑的）进程
    pub(super) async fn sdk_attach(self: &Arc<Self>, id: &str) -> Result<Arc<Proc>, LocalError> {
        let inst = self.inst(id);
        if let Some(p) = lock(&inst.st).sdk.proc.clone().filter(|p| p.alive()) {
            return Ok(p);
        }
        // 对话页要用了（发消息、换模型……）：先从终端收回来。收回来和拿到锁之间可能又交给了终端
        // （交给终端也拿这把锁）：拿到锁后再看一次，不然看看起的进程和终端里的 TUI 会同时写这段对话
        let mut tries = 0;
        let _g = loop {
            self.sdk_take_back(&inst).await?;
            let g = self.sdk.start.lock().await;
            if lock(&inst.st).sdk.term.is_none() {
                break g;
            }
            tries += 1;
            if tries >= 3 {
                return Err(LocalError::new("CHAT_TERMINAL_BUSY"));
            }
        };
        if let Some(p) = lock(&inst.st).sdk.proc.clone().filter(|p| p.alive()) {
            return Ok(p);
        }
        let row = self.store.instance(id)?.ok_or_else(LocalError::not_found)?;
        let opts = self.instances.opts(id);
        let profile = crate::claude_profiles::for_instance(&self.store, id);
        let profile_env = crate::claude_profiles::launch_env(&self.paths.home, &profile);
        if let Err(e) = crate::claude_profiles::skip_first_run(&self.paths.home, &profile, std::path::Path::new(&row.workdir)) {
            tracing::debug!(error = %e, "没能写好 Claude Code 的首次启动确认");
        }
        let shell = self.instances.shell().to_string();
        let program = claude_sdk::program(&shell).await.ok_or_else(|| LocalError::new("CLAUDE_NOT_FOUND"))?;
        let mut args: Vec<String> = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--permission-prompt-tool", "stdio"].map(String::from).to_vec();
        // 助理的大脑：权限只由闸门（PreToolUse）管，不能跳过权限，也不让切到跳过权限
        let brain = crate::assistant::is_brain(&self.store, id);
        // 跑腿和大脑一样只由闸门管权限
        let gated = brain || crate::assistant::is_errand(&self.store, id);
        if !gated {
            args.push(if opts.full_access { "--dangerously-skip-permissions" } else { "--allow-dangerously-skip-permissions" }.into());
        }
        if let Ok(p) = super::hooks::ensure_terminal_settings(&self.paths) {
            args.extend(["--settings".into(), p.display().to_string()]);
        }
        let (resume, resume_at, first, model, effort, mode, transcript) = {
            let mut st = lock(&inst.st);
            let resume = st.sdk.resume.take().or_else(|| st.binding.as_ref().filter(|b| b.transcript_path.is_file()).map(|b| (b.session_id.clone(), false)));
            let transcript = st.binding.as_ref().filter(|b| resume.as_ref().is_some_and(|(sid, _)| *sid == b.session_id)).map(|b| b.transcript_path.clone());
            (resume, st.sdk.resume_at.take(), st.sdk.first.take(), st.sdk.model.clone(), st.sdk.effort.clone(), st.sdk.mode.clone(), transcript)
        };
        // 接着的是界面上显示的那一支（编辑重发分出来的）：见 [`pin_leaf`]
        if let (Some((sid, _)), Some(path)) = (&resume, &transcript) {
            if let Err(e) = pin_leaf(path, sid) {
                tracing::warn!(error = %e, "没能把要接着的那一支记进 transcript");
            }
        }
        if let Some((sid, fork)) = &resume {
            args.extend(["--resume".into(), sid.clone()]);
            if *fork {
                args.push("--fork-session".into());
            }
            if let Some(at) = resume_at {
                args.extend(["--resume-session-at".into(), at]);
            }
        }
        // 助理的大脑（assistant.rs）：带上规矩和只给它的工具；没选过模型时用好的
        if brain {
            args.extend(crate::assistant::brain_args(&self.paths));
        } else if gated {
            args.extend(crate::assistant::errand_args());
        } else if crate::assistant::is_worker(&self.store, id) {
            // 助理开的会话：多一个找助理的工具（assistant_note）
            args.extend(crate::assistant::worker_args(&self.paths, id));
        }
        if let Some(m) = model.or_else(|| brain.then(|| crate::assistant::DEFAULT_MODEL.to_string())) {
            args.extend(["--model".into(), m]);
        }
        if let Some(e) = effort {
            args.extend(["--effort".into(), e]);
        }
        if let Some(m) = if gated { Some("default".to_string()) } else { mode } {
            args.extend(["--permission-mode".into(), m]);
        }
        // 对话里发的附件（图片以外的文件只给路径）放在附件目录，不在项目里：加进可访问的目录，读的时候不用一个个确认
        let attach = self.paths.attach_dir(id);
        if std::fs::create_dir_all(&attach).is_ok() {
            args.extend(["--add-dir".into(), attach.display().to_string()]);
        }
        let mut env = sdk_env(&shell, super::codex_app::user_env(&shell).await.unwrap_or_default());
        // root 下 claude 不肯带全部权限运行，要说明是在沙箱里（和终端里的启动命令一样）
        if crate::platform::is_root() {
            env.push(("IS_SANDBOX".to_string(), "1".to_string()));
        }
        let path = super::codex_app::launch_path(&shell, &program).await;
        let (tx, rx) = mpsc::unbounded_channel();
        tracing::info!(id, program = %program.display(), resume = ?resume.as_ref().map(|r| &r.0), "起对话模式的 Claude Code（stream-json）");
        let proc = Proc::spawn(claude_sdk::Spawn { program, args, cwd: PathBuf::from(&row.workdir), env, profile_env, path }, tx)?;
        tokio::spawn(self.clone().sdk_loop(inst.clone(), proc.clone(), rx));
        let fast = lock(&inst.st).sdk.fast.unwrap_or_else(|| account_fast(&crate::claude_profiles::config_paths(&self.paths.home, &profile).0));
        let init = match proc.request("initialize", json!({ "hooks": claude_sdk::hooks_payload(), "promptSuggestions": true })).await {
            Ok(v) => v,
            Err(e) => {
                proc.kill();
                let mut st = lock(&inst.st);
                st.error = Some(e.params.get("detail").and_then(Value::as_str).map(String::from).unwrap_or_else(|| e.code.to_string()).chars().take(500).collect());
                Self::set_phase(&inst, &mut st, Phase::Detached, Some("claude_sdk"));
                return Err(e);
            }
        };
        {
            let mut st = lock(&inst.st);
            if let Some(v) = init["claude_code_version"].as_str() {
                st.version = Some(v.to_string());
            }
            if let Some(m) = init["current_permission_mode"].as_str() {
                st.mode = Some(m.to_string());
            }
            st.sdk.fast_on = matches!(init["fast_mode_state"].as_str(), Some("on" | "cooldown"));
            st.sdk.proc = Some(proc.clone());
            st.sdk.idle_since = Some(Instant::now());
            st.error = None;
            if st.phase == Phase::Detached {
                Self::set_phase(&inst, &mut st, Phase::Idle, None);
            } else {
                Self::emit_status(&inst, &st);
            }
        }
        // 快速模式：-p 下要宿主明确打开（不然报“Agent SDK 里不能用”），打开就是开着。账号设置里开着（TUI 里 /fast 过）、
        // 或者这段对话里 /fast 打开过的，跟终端里一样开着
        if fast {
            if let Err(e) = proc.request("apply_flag_settings", json!({ "settings": { "fastMode": true } })).await {
                tracing::warn!(id, error = ?e, "没能打开快速模式");
            }
        }
        if let Some(text) = first {
            if let Err(e) = proc.send_user(json!(text)).await {
                tracing::warn!(id, error = ?e, "第一句话没发出去");
            } else {
                self.sdk_running(&inst);
            }
        }
        Ok(proc)
    }

    /// 给 stream-json 的对话起名：照 TUI 的做法用快速模型（`haiku`，第三方接口按它的 Haiku 档）读用户说过的话起一个短标题，
    /// 以 `ai-title` 记录追加到 transcript——终端名、对话页标题、历史列表、`claude --resume` 都认它。
    /// 用户 /rename 过（`custom-title`）的不动。起名用的这次不留对话记录、不带工具、不读用户的设置（不触发 hooks）。
    async fn sdk_auto_title(self: Arc<Self>, inst: Arc<Inst>, b: super::Binding) {
        let title = self.sdk_make_title(&inst.id, &b).await;
        let mut st = lock(&inst.st);
        st.sdk.titling = false;
        st.sdk.title_at = Some(Instant::now());
        st.sdk.title_turns = 0;
        drop(st);
        let Some(title) = title else { return };
        let line = json!({ "type": "ai-title", "aiTitle": title, "sessionId": b.session_id }).to_string() + "\n";
        use std::io::Write;
        let r = std::fs::OpenOptions::new().append(true).open(&b.transcript_path).and_then(|mut f| f.write_all(line.as_bytes()));
        match r {
            Ok(()) => tracing::info!(id = %inst.id, title, "给对话起了名字"),
            Err(e) => tracing::debug!(error = %e, "起的名字没写进 transcript"),
        }
    }

    async fn sdk_make_title(&self, id: &str, b: &super::Binding) -> Option<String> {
        let path = b.transcript_path.clone();
        let prompts = tokio::task::spawn_blocking(move || title_source(&std::fs::read_to_string(path).ok()?)).await.ok()??;
        let row = self.store.instance(id).ok()??;
        let profile = crate::claude_profiles::for_instance(&self.store, id);
        let profile_env = crate::claude_profiles::launch_env(&self.paths.home, &profile);
        let shell = self.instances.shell().to_string();
        let program = claude_sdk::program(&shell).await?;
        let mut cmd = claude_sdk::command(&program);
        cmd.args(["-p", "--model", "haiku", "--output-format", "json", "--no-session-persistence", "--setting-sources", "", "--strict-mcp-config", "--tools", "", "--disable-slash-commands"]);
        cmd.current_dir(&row.workdir).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        if let Some(p) = super::codex_app::launch_path(&shell, &program).await {
            cmd.env("PATH", p);
        }
        cmd.env_remove(super::hooks::INSTANCE_ENV).env_remove("TMUX").env_remove("TMUX_PANE");
        if let Some(e) = &profile_env {
            e.apply(&mut cmd);
        }
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        use tokio::io::AsyncWriteExt;
        stdin.write_all(title_prompt(&prompts).as_bytes()).await.ok()?;
        drop(stdin);
        let out = tokio::time::timeout(TITLE_WAIT, child.wait_with_output()).await.ok()?.ok()?;
        let v: Value = serde_json::from_slice(&out.stdout).ok()?;
        if v["is_error"] == true {
            tracing::debug!(result = %v["result"], "起名失败");
            return None;
        }
        parse_title(v["result"].as_str()?)
    }

    /// 结束进程（删除终端、换账号、重启）
    pub(super) fn sdk_stop(&self, inst: &Inst) {
        let p = {
            let mut st = lock(&inst.st);
            // 换了对话、换了账号：不再替旧的那段等额度重置
            self.limit_set(inst, &mut st, None);
            st.sdk.proc.take()
        };
        if let Some(p) = p {
            p.kill();
        }
    }

    /// 发出去了：马上显示成运行中（UserPromptSubmit 稍后到）
    fn sdk_running(&self, inst: &Inst) {
        let mut st = lock(&inst.st);
        st.sdk.interrupted = false;
        st.sdk.idle_since = None;
        // 自己发了消息（或者是到点发的 continue）：不再等
        self.limit_set(inst, &mut st, None);
        if st.phase == Phase::Idle {
            st.error = None;
            Self::set_phase(inst, &mut st, Phase::Running, None);
        }
        Self::guard_turn(&mut st);
    }

    /// 运行中、等审批时不自动更新
    fn guard_turn(st: &mut super::State) {
        let busy = matches!(st.phase, Phase::Running | Phase::Approval);
        match (busy, st.sdk.turn_guard.is_some()) {
            (true, false) => st.sdk.turn_guard = Some(crate::updater::TerminalGuard::new()),
            (false, true) => {
                st.sdk.turn_guard = None;
                st.sdk.idle_since = Some(Instant::now());
            }
            _ => {}
        }
    }

    async fn sdk_loop(self: Arc<Self>, inst: Arc<Inst>, proc: Arc<Proc>, mut rx: mpsc::UnboundedReceiver<Incoming>) {
        while let Some(m) = rx.recv().await {
            match m {
                Incoming::Message(v) => self.sdk_message(&inst, &proc, v),
                Incoming::Exited { code, stderr } => {
                    self.sdk_exited(&inst, &proc, code, &stderr);
                    break;
                }
            }
            Self::guard_turn(&mut lock(&inst.st));
        }
    }

    fn sdk_exited(&self, inst: &Inst, proc: &Arc<Proc>, code: Option<i32>, stderr: &str) {
        let mut st = lock(&inst.st);
        // 已经换成新的进程了（重启、换账号）：旧的退出不算
        if !st.sdk.proc.as_ref().is_none_or(|p| Arc::ptr_eq(p, proc)) {
            return;
        }
        st.sdk.proc = None;
        st.sdk.crons = false;
        st.sdk.tasks.clear();
        st.background.clear();
        st.agents.clear();
        st.compacting = None;
        Self::drop_pending(&mut st);
        // 交给终端时结束的：状态由“交给终端”定（终端里在接着）
        if st.sdk.term.is_some() {
            Self::guard_turn(&mut st);
            return;
        }
        let quick = !proc.killed() && proc.started_at.elapsed() < QUICK_EXIT && code != Some(0);
        tracing::info!(id = %inst.id, ?code, quick, "对话模式的 Claude Code 退出了");
        if quick || (!proc.killed() && code.is_some_and(|c| c != 0) && st.phase != Phase::Idle) {
            let tail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
            let tail = tail[tail.len().saturating_sub(6)..].join("\n");
            st.error = Some(if tail.is_empty() { format!("claude exited ({})", code.map_or("signal".into(), |c| c.to_string())) } else { tail.chars().take(800).collect() });
            Self::set_phase(inst, &mut st, Phase::Detached, Some("claude_sdk"));
        } else {
            // 正常退出（空闲时结束的、stdin 关了）：下次发消息时自动接着
            if st.phase != Phase::Idle {
                st.end_hint.get_or_insert(board::Outcome::Interrupted);
            }
            let next = if st.binding.is_some() || st.phase != Phase::Detached { Phase::Idle } else { Phase::Detached };
            Self::set_phase(inst, &mut st, next, None);
        }
        Self::guard_turn(&mut st);
    }

    fn sdk_message(self: &Arc<Self>, inst: &Arc<Inst>, proc: &Arc<Proc>, v: Value) {
        // 已经停掉的旧进程（/resume、/new、换账号、编辑重发时结束的）临走还会发 hook（SessionEnd、Stop……），
        // 带着旧的会话号：再交给 hook 处理就把刚换过去的对话又绑回旧的那段（/resume 后界面停在旧对话、历史里“当前”标错）
        if proc.killed() {
            return;
        }
        match v["type"].as_str().unwrap_or("") {
            "control_request" => self.sdk_control(inst, proc, v),
            "stream_event" => self.sdk_stream(inst, &v["event"], v["parent_tool_use_id"].is_null()),
            "system" => self.sdk_system(inst, &v),
            // 推荐回复（`initialize` 里开了 promptSuggestions）：输入框里灰色的那句，跟 TUI 里一样
            "prompt_suggestion" => {
                let s = v["suggestion"].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
                let mut st = lock(&inst.st);
                if st.suggestion != s {
                    st.suggestion = s;
                    Self::emit_status(inst, &st);
                }
            }
            "rate_limit_event" => {
                let info = &v["rate_limit_info"];
                let mut st = lock(&inst.st);
                st.sdk.limit_reset = if info["status"] == "rejected" { info["resetsAt"].as_i64().or(Some(0)) } else { None };
            }
            "assistant" if v["parent_tool_use_id"].is_null() => {
                // 一整条回复写完了，马上就会出现在 transcript 里：预览清掉，再推一次快照
                let mut st = lock(&inst.st);
                if st.live.text.take().is_some() {
                    Self::emit_status(inst, &st);
                }
            }
            "result" => {
                let mut st = lock(&inst.st);
                st.live = Default::default();
                if let Some(f) = v["fast_mode_state"].as_str() {
                    st.sdk.fast_on = matches!(f, "on" | "cooldown");
                }
                st.compacting = None;
                for (m, u) in v["modelUsage"].as_object().into_iter().flatten() {
                    if let Some(w) = u["contextWindow"].as_u64().filter(|&w| w > 0) {
                        st.sdk.windows.insert(models::base_id(m), w);
                    }
                }
                if v["is_error"] == true && v["subtype"] != "error_during_execution" {
                    let msg = v["result"].as_str().map(String::from).or_else(|| v["errors"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n")));
                    // StopFailure 先到，已经记下了是哪种错（billing_error、authentication_failed……）：保留它，
                    // 界面按它显示“换个账号继续”；换成报错原文就认不出来了
                    let code = st.error.take().filter(|e| is_error_code(e));
                    let text = msg.clone().unwrap_or_default();
                    st.error = code.or_else(|| msg.filter(|m| !m.is_empty()).map(|m| m.chars().take(500).collect()));
                    if is_expired_text(&text) {
                        // 第三方接口的套餐到期、余额用完：等多久都不会好，不自动继续，界面上让用户续费或换账号
                        st.sdk.limit_reset = None;
                        st.error = Some("quota_expired".into());
                    } else if st.sdk.limit_reset.is_some() || st.error.as_deref().is_some_and(|e| e == "rate_limit" || is_limit_text(e)) || is_limit_text(&text) {
                        // 额度用完：默认等重置后自动继续（跟 TUI 里 Claude Code 默认的一样），界面上可以取消、换账号。
                        // 重置时间：rate_limit_event 给的；没有时看报错原文里有没有写（第三方接口常把时间写在话里）
                        st.error = Some("rate_limit".into());
                        let reset = st.sdk.limit_reset.take().filter(|&t| t > 0).or_else(|| reset_in_text(&text, crate::util::system_ms() / 1000));
                        tokio::spawn(self.clone().sdk_limit_wait(inst.clone(), reset));
                    }
                }
                if st.sdk.interrupted {
                    st.end_hint = Some(board::Outcome::Interrupted);
                    st.sdk.interrupted = false;
                }
                // `claude -p` 不像 TUI 那样自己起名（transcript 里没有 ai-title）：看看照着做
                if st.error.is_none() {
                    st.sdk.limit_early = false;
                    st.sdk.limit_tries = 0;
                    st.sdk.title_turns += 1;
                    let due = st.sdk.title_at.is_none_or(|t| st.sdk.title_turns >= TITLE_TURNS && t.elapsed() >= TITLE_GAP);
                    if due && !st.sdk.titling {
                        if let Some(b) = st.binding.clone() {
                            st.sdk.titling = true;
                            tokio::spawn(self.clone().sdk_auto_title(inst.clone(), b));
                        }
                    }
                }
                if matches!(st.phase, Phase::Running | Phase::Approval) && st.pending.is_none() {
                    let outcome = if st.error.is_some() { board::Outcome::Failed } else { board::Outcome::Done };
                    st.end_hint.get_or_insert(outcome);
                    Self::set_phase(inst, &mut st, Phase::Idle, None);
                    // 打开终端标签时在跑的那一轮做完了：现在交给终端
                    if std::mem::take(&mut st.sdk.term_after) {
                        let (me, id) = (self.clone(), inst.id.clone());
                        tokio::spawn(async move {
                            if let Err(e) = me.sdk_to_terminal(&id).await {
                                tracing::warn!(id, code = %e.code, "这一轮做完后没能交给终端");
                            }
                        });
                    }
                } else {
                    Self::emit_status(inst, &st);
                }
            }
            _ => {}
        }
    }

    fn sdk_system(&self, inst: &Inst, v: &Value) {
        let mut st = lock(&inst.st);
        match v["subtype"].as_str().unwrap_or("") {
            "init" => {
                let mut changed = false;
                if let Some(m) = v["model"].as_str().filter(|m| st.model.as_deref() != Some(*m)) {
                    st.model = Some(m.to_string());
                    changed = true;
                }
                if let Some(m) = v["permissionMode"].as_str().filter(|m| st.mode.as_deref() != Some(*m)) {
                    st.mode = Some(m.to_string());
                    changed = true;
                }
                if let Some(ver) = v["claude_code_version"].as_str().filter(|x| st.version.as_deref() != Some(*x)) {
                    st.version = Some(ver.to_string());
                    changed = true;
                }
                st.sdk.ultrathink = v["ultrathink_typed_raises"] == true;
                if let Some(f) = v["fast_mode_state"].as_str() {
                    st.sdk.fast_on = matches!(f, "on" | "cooldown");
                }
                if changed {
                    Self::emit_status(inst, &st);
                }
            }
            "status" => {
                let activity = match v["status"].as_str() {
                    Some("compacting") => Some("compacting".to_string()),
                    _ => None,
                };
                if activity.is_some() {
                    st.compacting.get_or_insert_with(Instant::now);
                }
                if st.live.activity != activity {
                    st.live.activity = activity;
                    Self::emit_status(inst, &st);
                }
            }
            "compact_boundary" => {
                st.compacting = None;
                if st.live.activity.as_deref() == Some("compacting") {
                    st.live.activity = None;
                    Self::emit_status(inst, &st);
                }
            }
            "task_started" => {
                if let (Some(tool), Some(task)) = (v["tool_use_id"].as_str(), v["task_id"].as_str()) {
                    st.sdk.tasks.insert(tool.to_string(), task.to_string());
                }
            }
            "background_tasks_changed" => {
                let mut counts: Vec<(u32, String)> = vec![];
                for t in v["tasks"].as_array().into_iter().flatten() {
                    let kind = match t["task_type"].as_str().unwrap_or("") {
                        "local_bash" => "shell",
                        k if k.contains("agent") => "agent",
                        _ => "task",
                    };
                    match counts.iter_mut().find(|(_, k)| k == kind) {
                        Some(c) => c.0 += 1,
                        None => counts.push((1, kind.to_string())),
                    }
                }
                if st.background != counts {
                    st.background = counts;
                    Self::emit_status(inst, &st);
                }
            }
            "api_retry" => {
                let a = Some("retrying".to_string());
                if st.live.activity != a {
                    st.live.activity = a;
                    Self::emit_status(inst, &st);
                }
            }
            _ => {}
        }
    }

    /// 正在输出的回复：主线的文字增量拼起来（子代理的不算）
    fn sdk_stream(&self, inst: &Inst, ev: &Value, main: bool) {
        if !main {
            return;
        }
        let mut st = lock(&inst.st);
        match ev["type"].as_str().unwrap_or("") {
            "message_start" => {
                st.live.text = None;
            }
            "content_block_delta" => match ev["delta"]["type"].as_str() {
                Some("text_delta") => {
                    let t = st.live.text.get_or_insert_with(String::new);
                    t.push_str(ev["delta"]["text"].as_str().unwrap_or(""));
                    let n = t.chars().count();
                    if n > PREVIEW_LIMIT {
                        *t = t.chars().skip(n - PREVIEW_LIMIT).collect();
                    }
                    if st.live.activity.as_deref() == Some("thinking") || st.live.activity.as_deref() == Some("retrying") {
                        st.live.activity = None;
                    }
                }
                Some("thinking_delta") => {
                    if st.live.activity.is_none() {
                        st.live.activity = Some("thinking".into());
                    }
                }
                _ => return,
            },
            _ => return,
        }
        let due = st.sdk.preview_at.is_none_or(|t| t.elapsed() >= PREVIEW_GAP);
        if due {
            st.sdk.preview_at = Some(Instant::now());
            Self::emit_status(inst, &st);
        }
    }

    fn sdk_control(self: &Arc<Self>, inst: &Arc<Inst>, proc: &Arc<Proc>, v: Value) {
        let rid = v["request_id"].as_str().unwrap_or("").to_string();
        let req = v["request"].clone();
        let (me, inst, proc) = (self.clone(), inst.clone(), proc.clone());
        match req["subtype"].as_str().unwrap_or("") {
            "hook_callback" => {
                tokio::spawn(async move {
                    let mut input = req["input"].clone();
                    if input["hook_event_name"].is_null() {
                        if let Some(e) = req["callback_id"].as_str().and_then(|c| c.strip_prefix("ll:")) {
                            input["hook_event_name"] = json!(e);
                        }
                    }
                    if proc.killed() {
                        return;
                    }
                    if let Some(c) = input["session_crons"].as_array() {
                        lock(&inst.st).sdk.crons = !c.is_empty();
                    }
                    let out = me.hook(&inst.id, input).await;
                    let resp: Value = serde_json::from_str(&out).unwrap_or_else(|_| json!({}));
                    if let Err(e) = proc.respond(&rid, resp).await {
                        tracing::debug!(error = ?e, "hook 的答复没发出去");
                    }
                });
            }
            "can_use_tool" => {
                tokio::spawn(async move { me.sdk_permission(&inst, &proc, &rid, req).await });
            }
            other => {
                tracing::debug!(subtype = other, "不支持的控制请求");
                let other = other.to_string();
                tokio::spawn(async move {
                    let _ = proc.respond_error(&rid, &format!("unsupported: {other}")).await;
                });
            }
        }
    }

    /// 审批、提问、计划确认：挂起，等界面回答（没有超时：不再有 TUI 兜底）。没人回答就被放掉（停止、进程退出）时拒绝。
    async fn sdk_permission(&self, inst: &Arc<Inst>, proc: &Arc<Proc>, rid: &str, req: Value) {
        let tool = req["tool_name"].as_str().unwrap_or("").to_string();
        let input = req["input"].clone();
        let (tx, rx) = oneshot::channel();
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut st = lock(&inst.st);
            Self::drop_pending(&mut st);
            let kind = inst.agent.pending_kind(&tool);
            st.pending = Some(Pending { id, at: Instant::now(), tool: tool.clone(), kind, input: input.clone(), suggestions: req["permission_suggestions"].clone(), guard: None, tx: Some(tx) });
            Self::enter(&mut st, Phase::Approval);
            st.phase = Phase::Approval;
            st.reason = None;
            Self::guard_turn(&mut st);
            Self::emit_status(inst, &st);
        }
        let out = rx.await.unwrap_or_default();
        let mut resp: Value = serde_json::from_str(&out).unwrap_or_else(|_| json!({ "behavior": "deny", "message": "Nobody answered this request in Looklook." }));
        // allow 要带上（可能改过的）参数
        if resp["behavior"] == "allow" && resp.get("updatedInput").is_none() {
            resp["updatedInput"] = input;
        }
        {
            let mut st = lock(&inst.st);
            if st.pending.as_ref().is_some_and(|p| p.id == id) {
                st.pending = None;
            }
            if st.phase == Phase::Approval && st.pending.is_none() {
                Self::set_phase(inst, &mut st, Phase::Running, None);
            }
        }
        if let Err(e) = proc.respond(rid, resp).await {
            tracing::debug!(error = ?e, "审批的答复没发出去");
        }
    }

    /// 界面的回答 → `can_use_tool` 的答复（和 PermissionRequest hook 的 `decision` 同一种格式）
    pub(super) fn sdk_answer(&self, inst: &Inst, req: &PermissionAnswer) -> Result<(), LocalError> {
        let mut st = lock(&inst.st);
        let term = st.sdk.term.is_some();
        let Some(p) = st.pending.as_mut().filter(|p| p.id == req.id) else { return Err(LocalError::new("CHAT_STALE")) };
        let out = if p.guard.is_some() {
            // 命令规则的确认：答复的是 PreToolUse hook
            match req.decision.as_str() {
                "allow" => super::pre_tool_use("allow", "The user confirmed this command in Looklook."),
                "deny" => {
                    let why = req.message.as_deref().map(str::trim).filter(|m| !m.is_empty());
                    super::pre_tool_use("deny", &format!("The user declined this command in Looklook.{}", why.map(|m| format!(" Their note: {m}")).unwrap_or_default()))
                }
                _ => return Err(LocalError::invalid("decision", "invalid")),
            }
        } else if term {
            // 交给终端了：挂着的是终端里 TUI 的 PermissionRequest hook（见 Chat::hook），答复照 hook 的格式
            if req.decision == "terminal" {
                p.resolve(String::new());
                Self::settle(inst, &mut st, Phase::Tui);
                return Ok(());
            }
            let decision = inst.agent.decision(&super::PendingView { tool: &p.tool, input: &p.input, suggestions: &p.suggestions }, p.kind, req)?;
            json!({ "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision } }).to_string()
        } else {
            if req.decision == "terminal" {
                return Err(LocalError::invalid("decision", "invalid"));
            }
            inst.agent.decision(&super::PendingView { tool: &p.tool, input: &p.input, suggestions: &p.suggestions }, p.kind, req)?.to_string()
        };
        p.resolve(out);
        Self::settle(inst, &mut st, Phase::Running);
        Ok(())
    }

    /// 计划确认：1 = 批准并自动接受改动，2 = 批准、改动逐个确认，3 = 不批准，按反馈改计划
    pub(super) fn sdk_plan(&self, inst: &Inst, choice: u8, feedback: Option<&str>) -> Result<(), LocalError> {
        let mut st = lock(&inst.st);
        let Some(p) = st.pending.as_mut().filter(|p| p.kind == "plan") else { return Err(LocalError::new("CHAT_STALE")) };
        let set_mode = |mode: &str| json!([{ "type": "setMode", "mode": mode, "destination": "session" }]);
        let out = match choice {
            1 => json!({ "behavior": "allow", "updatedInput": p.input, "updatedPermissions": set_mode("acceptEdits") }),
            2 => json!({ "behavior": "allow", "updatedInput": p.input, "updatedPermissions": set_mode("default") }),
            3 => {
                let f = feedback.map(str::trim).filter(|f| !f.is_empty()).ok_or_else(|| LocalError::invalid("feedback", "required"))?;
                json!({ "behavior": "deny", "message": format!("The user wants changes to the plan: {}", f.chars().take(2000).collect::<String>()) })
            }
            _ => return Err(LocalError::invalid("choice", "invalid")),
        };
        p.resolve(out.to_string());
        if choice != 3 {
            st.mode = Some(if choice == 1 { "acceptEdits" } else { "default" }.into());
        }
        Self::settle(inst, &mut st, Phase::Running);
        Ok(())
    }

    /// 发消息。`/new` 是 `/clear` 的别名；会话记的账号不是现在的默认时，`/new`、`/clear` 换成默认账号重新开一段
    pub(super) async fn sdk_input(self: &Arc<Self>, id: &str, text: &str, images: &[String]) -> Result<Value, LocalError> {
        let inst = self.inst(id);
        lock(&inst.st).sdk.sending += 1;
        let r = self.sdk_input_inner(&inst, id, text, images).await;
        lock(&inst.st).sdk.sending -= 1;
        r
    }

    async fn sdk_input_inner(self: &Arc<Self>, inst: &Arc<Inst>, id: &str, text: &str, images: &[String]) -> Result<Value, LocalError> {
        let inst = inst.clone();
        if matches!(lock(&inst.st).phase, Phase::Approval) && !text.starts_with('/') {
            // 审批挂着时发的消息 claude 要等审批答完才处理：先提示用户回答
            return Err(LocalError::new("CHAT_BUSY"));
        }
        // 在对话页发了消息：打开终端标签时说好的“这一轮做完交给终端”不算了
        lock(&inst.st).sdk.term_after = false;
        let text = if text == "/new" { "/clear" } else { text };
        // 换到以前的一段对话：`/resume` 只有交互界面有，这里是重起进程 `--resume`
        if let Some(sid) = text.strip_prefix("/resume ").map(str::trim).filter(|s| crate::history::valid_id(s)) {
            self.sdk_resume(id, sid).await?;
            return Ok(json!({ "delivered": true }));
        }
        if text == "/clear" && images.is_empty() && self.sdk_new_with_default(id).await? {
            return Ok(json!({ "delivered": true }));
        }
        let proc = self.sdk_attach(id).await?;
        // 手打的 `/fast`：-p 下 claude 只认宿主打开的快速模式（flag settings），自己切不了。看看替它切，
        // 再把 `/fast on|off` 发过去，让 claude 照常回一句开了没有（额度不够、模型不支持时是它的原话）
        let fast = match text.trim().to_ascii_lowercase().as_str() {
            "/fast" => Some(!lock(&inst.st).sdk.fast_on),
            "/fast on" => Some(true),
            "/fast off" => Some(false),
            _ => None,
        };
        if let Some(on) = fast {
            proc.request("apply_flag_settings", json!({ "settings": { "fastMode": on } })).await?;
            lock(&inst.st).sdk.fast = Some(on);
            proc.send_user(json!(if on { "/fast on" } else { "/fast off" })).await?;
            return Ok(json!({ "delivered": true }));
        }
        let content = claude_sdk::user_content(text, images)?;
        // 手打了 ultrathink：TUI 里这一轮按 xhigh 想；-p 下要宿主说明是人打的（粘贴的不算，看看的输入框就是手打）
        let ultrathink = lock(&inst.st).sdk.ultrathink && text.to_lowercase().contains("ultrathink");
        proc.send_user_typed(content, ultrathink).await?;
        // 手打的 `/model sonnet`、`/effort high`：claude 只在这个进程里生效，跟界面上选的一样记下来，进程重起时带上
        // （不然空闲结束、换账号、重启客户端后就回去了）
        // 也算“上次选的”，下次新建对话时当默认（model_memory.rs）
        if let Some((cmd, v)) = text.split_once(' ').map(|(c, v)| (c, v.trim())).filter(|(_, v)| !v.is_empty() && !v.contains(char::is_whitespace)) {
            let ok_model = v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || "._-[]".contains(c));
            let kind = {
                let mut st = lock(&inst.st);
                match cmd {
                    "/model" if ok_model => {
                        st.model = Some(v.to_string());
                        st.model_set_at = Some(st.model_msg.clone());
                        st.sdk.model = Some(v.to_string());
                        Some("model")
                    }
                    "/effort" if super::screen::EFFORTS.contains(&v) => {
                        st.effort = Some(v.to_string());
                        st.sdk.effort = Some(v.to_string());
                        Some("effort")
                    }
                    _ => None,
                }
            };
            if let Some(kind) = kind {
                self.remember_model(id, kind, v);
            }
        }
        if !text.starts_with('/') {
            self.sdk_running(&inst);
        }
        Ok(json!({ "delivered": true }))
    }

    /// 立即发送（运行中）：跟终端里一样——先把前台在跑的命令、子代理转到后台（`background_tasks`，Ctrl+B），
    /// 再发 `priority: "now"` 的一条：claude 停下这一轮先回答它，转到后台的命令跑完后它会再读结果。
    /// `text` 为空：只把命令转到后台，这一轮马上接着走，排着的那些也就马上并进来。
    pub(super) async fn sdk_send_now(self: &Arc<Self>, id: &str, text: &str, images: &[String]) -> Result<Value, LocalError> {
        let inst = self.inst(id);
        if !matches!(lock(&inst.st).phase, Phase::Running) || text.starts_with('/') {
            return self.sdk_input(id, text, images).await;
        }
        let proc = self.sdk_attach(id).await?;
        // 老版本不认这个请求时照样发（只是要等命令跑完）
        if let Err(e) = proc.request("background_tasks", json!({})).await {
            tracing::debug!(id, error = ?e, "立即发送：background_tasks 不可用");
        }
        if text.is_empty() && images.is_empty() {
            return Ok(json!({ "delivered": true }));
        }
        lock(&inst.st).sdk.term_after = false;
        let content = claude_sdk::user_content(text, images)?;
        let ultrathink = lock(&inst.st).sdk.ultrathink && text.to_lowercase().contains("ultrathink");
        proc.send_user_with(content, ultrathink, true).await?;
        Ok(json!({ "delivered": true }))
    }

    /// `/new`、`/clear` 时会话记的账号不是默认的：改用默认账号，开一段新的
    async fn sdk_new_with_default(self: &Arc<Self>, id: &str) -> Result<bool, LocalError> {
        let def = crate::claude_profiles::default_id(&self.store);
        if crate::claude_profiles::pinned(&self.store, id).is_none_or(|p| p == def) || self.instances.container(id).is_some() {
            return Ok(false);
        }
        tracing::info!(id, profile = %def, "/new：会话记的账号不是默认的，按默认账号重新开");
        crate::claude_profiles::pin(&self.store, id, None)?;
        self.sdk_fresh(id).await?;
        Ok(true)
    }

    /// 换到以前的一段对话：结束进程，`--resume` 那一段重新起
    async fn sdk_resume(self: &Arc<Self>, id: &str, session: &str) -> Result<(), LocalError> {
        let inst = self.inst(id);
        self.sdk_stop(&inst);
        let path = claude_sdk::find_transcript(&self.claude_config_dirs(), session);
        {
            let mut st = lock(&inst.st);
            st.sdk.resume = Some((session.to_string(), false));
            st.binding = path.map(|p| super::Binding { session_id: session.to_string(), transcript_path: p });
            if let Some(b) = &st.binding {
                let _ = self.store.set(&super::binding_key(id), b);
                crate::history::mark_own(&self.store, session);
            }
            st.title = None;
            st.board.title = None;
            st.board.first_prompt = None;
            st.board_dirty = true;
            Self::set_phase(&inst, &mut st, Phase::Idle, None);
        }
        *inst.tr.lock().await = None;
        let _ = inst.tx.send(Event::Reset);
        self.sdk_attach(id).await.map(|_| ())
    }

    /// 编辑重发（[`Chat::edit`]）：结束进程，同一个会话 `--resume-session-at <at>` 重新起，之后发的话接在 `at` 后面。
    /// `at` 为空（改的是第一句话）：开一段新的。
    pub(super) async fn sdk_edit(self: &Arc<Self>, id: &str, at: Option<String>) -> Result<(), LocalError> {
        let inst = self.inst(id);
        let Some(at) = at else { return self.sdk_fresh(id).await };
        let sid = lock(&inst.st).binding.as_ref().map(|b| b.session_id.clone()).ok_or_else(|| LocalError::new("CHAT_EDIT_STALE"))?;
        self.sdk_stop(&inst);
        {
            let mut st = lock(&inst.st);
            st.sdk.resume = Some((sid, false));
            st.sdk.resume_at = Some(at);
            Self::set_phase(&inst, &mut st, Phase::Idle, None);
        }
        self.sdk_attach(id).await.map(|_| ())
    }

    /// 结束进程、忘掉当前这段对话，起一个新的
    pub(super) async fn sdk_fresh(self: &Arc<Self>, id: &str) -> Result<(), LocalError> {
        let inst = self.inst(id);
        self.sdk_stop(&inst);
        {
            let mut st = lock(&inst.st);
            st.binding = None;
            st.title = None;
            st.board.title = None;
            st.board.first_prompt = None;
            st.board_dirty = true;
            Self::set_phase(&inst, &mut st, Phase::Idle, None);
        }
        let _ = self.store.remove(&super::binding_key(id));
        // 新建时接着的那段也忘掉：不然还没说话（没有新的对话记录）时，会话列表按它显示旧对话的名字
        let _ = self.store.remove(&crate::instances::resume_key(id));
        *inst.tr.lock().await = None;
        let _ = inst.tx.send(Event::Reset);
        self.sdk_attach(id).await.map(|_| ())
    }

    /// 额度用完：算出什么时候重置（`reset`，Unix 秒；不知道时看用量接口，再不知道就过一会儿试一次）再多等一会儿，记下来，
    /// 到点由 [`Chat::sdk_limit_tick`] 发 continue
    async fn sdk_limit_wait(self: Arc<Self>, inst: Arc<Inst>, reset: Option<i64>) {
        let reset = match reset {
            Some(t) => Some(t),
            None => self.usage(&inst.id).await.ok().and_then(|u| limit_reset_in(&u)),
        };
        let now = crate::util::system_ms();
        let mut st = lock(&inst.st);
        // 取用量的这会儿已经有人接手了（发了消息、换了账号）
        if st.phase != Phase::Idle || st.error.as_deref() != Some("rate_limit") {
            return;
        }
        let guess = LIMIT_RETRY_MS[(st.sdk.limit_tries as usize).min(LIMIT_RETRY_MS.len() - 1)];
        let at = reset.map_or(now + guess, |t| (t * 1000).max(now) + LIMIT_SLACK_MS);
        st.sdk.limit_guess = reset.is_none();
        if reset.is_none() {
            st.sdk.limit_tries += 1;
        }
        tracing::info!(id = %inst.id, at, "额度用完了，等重置后自动继续");
        let poll = !std::mem::take(&mut st.sdk.limit_early);
        st.sdk.limit_since = now;
        st.sdk.limit_poll = poll.then_some(now + LIMIT_POLL_MS);
        self.limit_set(&inst, &mut st, Some(at));
    }

    /// 记下（存进数据库）或取消“额度重置后自动继续”
    fn limit_set(&self, inst: &Inst, st: &mut super::State, at: Option<i64>) {
        if st.sdk.limit_resume == at {
            return;
        }
        st.sdk.limit_resume = at;
        let key = limit_key(&inst.id);
        let _ = match at {
            Some(t) => self.store.set(&key, &t),
            None => self.store.remove(&key),
        };
        Self::guard_turn(st);
        Self::emit_status(inst, st);
    }

    /// 后台每隔几秒看一次（[`Chat::background`]）：在等额度重置的到点了就发 continue。
    /// 进程空闲时被结束了也没关系，发消息时会 `--resume` 接着
    pub(super) fn sdk_limit_tick(self: &Arc<Self>, inst: &Arc<Inst>) {
        let mut st = lock(&inst.st);
        let Some(at) = st.sdk.limit_resume else { return };
        let now = crate::util::system_ms();
        if st.sdk.limit_firing {
            return;
        }
        if now < at {
            if st.sdk.limit_poll.is_some_and(|p| now >= p) && !st.sdk.limit_checking && matches!(st.phase, Phase::Idle | Phase::Detached) {
                st.sdk.limit_checking = true;
                drop(st);
                tokio::spawn(self.clone().sdk_limit_check(inst.clone()));
            }
            return;
        }
        if !matches!(st.phase, Phase::Idle | Phase::Detached) {
            self.limit_set(inst, &mut st, None);
            return;
        }
        st.sdk.limit_firing = true;
        drop(st);
        let (me, inst) = (self.clone(), inst.clone());
        tokio::spawn(async move {
            tracing::info!(id = %inst.id, "额度应该重置了，自动继续");
            let r = me.sdk_input(&inst.id, "continue", &[]).await;
            let mut st = lock(&inst.st);
            st.sdk.limit_firing = false;
            if let Err(e) = r {
                tracing::warn!(code = %e.code, "额度重置后没能自动继续");
            }
            me.limit_set(&inst, &mut st, None);
        });
    }

    /// 等重置期间看一眼用量：用完的窗口都回来了就把自动继续提前到现在（下一次 [`Chat::sdk_limit_tick`] 发）。
    /// 用量是 Claude Code 自己的缓存，旧了会在后台去取，这次看不到下次（[`LIMIT_POLL_MS`] 后）就看到了
    async fn sdk_limit_check(self: Arc<Self>, inst: Arc<Inst>) {
        let u = self.usage(&inst.id).await.ok();
        let now = crate::util::system_ms();
        let mut st = lock(&inst.st);
        st.sdk.limit_checking = false;
        if st.sdk.limit_resume.is_none_or(|at| at <= now) || st.sdk.limit_poll.is_none() {
            return;
        }
        st.sdk.limit_poll = Some(now + LIMIT_POLL_MS);
        if u.as_ref().is_some_and(|u| limit_back(u, st.sdk.limit_since)) {
            tracing::info!(id = %inst.id, "还没到重置时间，但用量里额度已经回来了，提前自动继续");
            st.sdk.limit_early = true;
            self.limit_set(&inst, &mut st, Some(now));
        }
    }

    /// 客户端启动时：重启前在等额度重置的会话接着等
    pub(super) fn sdk_limit_restore(&self) {
        for r in self.store.instances().unwrap_or_default() {
            let key = limit_key(&r.id);
            let Ok(Some(at)) = self.store.get::<i64>(&key) else { continue };
            if !self.instances.claude_sdk_mode(&r) {
                let _ = self.store.remove(&key);
                continue;
            }
            let inst = self.inst(&r.id);
            let mut st = lock(&inst.st);
            st.sdk.limit_resume = Some(at);
            let now = crate::util::system_ms();
            st.sdk.limit_since = now;
            st.sdk.limit_poll = Some(now + LIMIT_POLL_MS);
            st.error = Some("rate_limit".into());
            Self::guard_turn(&mut st);
        }
    }

    pub(super) async fn sdk_interrupt(&self, id: &str) -> Result<(), LocalError> {
        let inst = self.inst(id);
        // 在等额度重置自动继续：“不自动继续”
        {
            let mut st = lock(&inst.st);
            if st.sdk.limit_resume.is_some() {
                self.limit_set(&inst, &mut st, None);
                return Ok(());
            }
        }
        // 交给终端了：停的是终端里的 TUI（Esc 中断这一轮）
        if lock(&inst.st).sdk.term.is_some() {
            if matches!(lock(&inst.st).phase, Phase::Running | Phase::Approval) {
                self.instances.send_keys(id, &["Escape"]).await?;
            }
            return Ok(());
        }
        let Some(proc) = lock(&inst.st).sdk.proc.clone().filter(|p| p.alive()) else { return Ok(()) };
        {
            let mut st = lock(&inst.st);
            st.sdk.interrupted = true;
            Self::drop_pending(&mut st);
            Self::stop_tool(&inst, &mut st);
        }
        let r = proc.request("interrupt", json!({})).await.map(|_| ());
        // 正常会马上收到这一轮的 result（→ 空闲）；卡住没来就自己收尾，别一直转圈
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(8)).await;
            let mut st = lock(&inst.st);
            if st.sdk.interrupted && matches!(st.phase, Phase::Running | Phase::Approval) {
                Self::drop_pending(&mut st);
                Self::set_phase(&inst, &mut st, Phase::Idle, None);
            }
        });
        r
    }

    /// 快捷设置：模型、思考强度、权限模式，运行中也能改（下一次请求生效）
    pub(super) async fn sdk_setting(self: &Arc<Self>, id: &str, kind: &str, value: &str) -> Result<(), LocalError> {
        let inst = self.inst(id);
        match kind {
            "model" => {
                if value.is_empty() || value.len() > 64 || !value.chars().all(|c| c.is_ascii_alphanumeric() || "._-[]".contains(c)) {
                    return Err(LocalError::invalid("value", "invalid"));
                }
            }
            "effort" => {
                if !super::screen::EFFORTS.contains(&value) {
                    return Err(LocalError::invalid("value", "invalid"));
                }
            }
            "mode" => {
                const MODES: [&str; 5] = ["default", "acceptEdits", "plan", "auto", "bypassPermissions"];
                if !MODES.contains(&value) {
                    return Err(LocalError::invalid("value", "invalid"));
                }
                // 助理的大脑、跑腿：权限由助理的权限模式定（助理页上换）
                if crate::assistant::is_gated(&self.store, id) {
                    return Err(LocalError::new("CHAT_MODE_UNAVAILABLE"));
                }
            }
            _ => return Err(LocalError::invalid("kind", "invalid")),
        }
        let proc = self.sdk_attach(id).await?;
        match kind {
            "model" => {
                proc.request("set_model", json!({ "model": value })).await?;
            }
            // `max` 只能在这次会话里用（存不进设置），所以走启动参数那一套的 flag settings
            "effort" => {
                proc.request("apply_flag_settings", json!({ "settings": { "effortLevel": value } })).await?;
            }
            _ => {
                proc.request("set_permission_mode", json!({ "mode": value })).await.map_err(|e| if e.code == "CLAUDE_SDK_ERROR" { LocalError::new("CHAT_MODE_UNAVAILABLE") } else { e })?;
            }
        }
        let mut st = lock(&inst.st);
        match kind {
            "model" => {
                st.model = Some(value.to_string());
                st.model_set_at = Some(st.model_msg.clone());
                st.sdk.model = Some(value.to_string());
            }
            "effort" => {
                st.effort = Some(value.to_string());
                st.sdk.effort = Some(value.to_string());
            }
            _ => {
                st.mode = Some(value.to_string());
                st.sdk.mode = Some(value.to_string());
            }
        }
        Self::emit_status(&inst, &st);
        Ok(())
    }

    /// 停止一个后台任务（`stop_task`）。task id 来自 `task_started`，客户端重启后从工具结果里认
    /// （`Command running in background with ID: <id>`）。
    pub(super) async fn sdk_stop_task(&self, id: &str, tool: &str) -> Result<(), LocalError> {
        let inst = self.inst(id);
        let proc = lock(&inst.st).sdk.proc.clone().filter(|p| p.alive()).ok_or_else(|| LocalError::new("CHAT_NOT_RUNNING"))?;
        let mut task = lock(&inst.st).sdk.tasks.get(tool).cloned();
        if task.is_none() {
            let tr = inst.tr.lock().await;
            let t = tr.as_ref().and_then(super::Reader::claude).ok_or_else(LocalError::not_found)?;
            task = t.tool_calls().into_iter().find(|c| c.id == tool).and_then(|c| match c.result.map(|r| &r.body) {
                Some(super::transcript::Body::ToolResult { content, .. }) => task_id_in(content),
                _ => None,
            });
        }
        let task = task.ok_or_else(LocalError::not_found)?;
        proc.request("stop_task", json!({ "task_id": task })).await.map(|_| ())
    }

    /// 换账号：结束进程，改用 `profile`，接着同一段对话起来；`go_on` 时起来后发 continue
    pub(super) async fn sdk_switch_profile(self: &Arc<Self>, id: &str, profile: &str, go_on: bool) -> Result<(), LocalError> {
        let inst = self.inst(id);
        lock(&inst.st).binding.as_ref().ok_or_else(|| LocalError::new("CHAT_NOT_RUNNING"))?;
        crate::claude_profiles::pin(&self.store, id, Some(profile))?;
        self.sdk_stop(&inst);
        {
            // 换了账号就不用再等原来那个账号的额度了（不然到点还会再发一次 continue）
            let mut st = lock(&inst.st);
            st.error = None;
            st.sdk.limit_tries = 0;
            self.limit_set(&inst, &mut st, None);
            Self::set_phase(&inst, &mut st, Phase::Idle, None);
        }
        self.sdk_attach(id).await?;
        if go_on {
            self.sdk_input(id, "continue", &[]).await?;
        }
        Ok(())
    }

    /// 重新起进程（界面上的“重新启动”）
    pub(super) async fn sdk_relaunch(self: &Arc<Self>, id: &str) -> Result<(), LocalError> {
        let inst = self.inst(id);
        self.sdk_stop(&inst);
        {
            let mut st = lock(&inst.st);
            st.error = None;
            Self::set_phase(&inst, &mut st, Phase::Detached, Some("starting"));
        }
        self.sdk_attach(id).await.map(|_| ())
    }

    // ---------------- 交给终端（docs/CLAUDE_SDK.md「终端里接着」） ----------------

    /// 打开终端标签：对话交给终端——结束看看起的进程，在终端的 shell 里打开 `claude --resume <这段对话>`。
    /// 一段对话同时只能有一个 Claude Code 在写，所以是交接不是同步：对话页照常跟读记录（只看），
    /// 在对话页发消息时收回来。返回 `state`：`terminal` 交过去了（或者早就交了）/ `after_turn` 这一轮做完再交 /
    /// `no_session` 还没聊过（终端里留着 shell）/ `busy` 终端里有别的程序在跑
    pub(super) async fn sdk_to_terminal(self: &Arc<Self>, id: &str) -> Result<Value, LocalError> {
        let inst = self.inst(id);
        let b = {
            let mut st = lock(&inst.st);
            if st.sdk.term.is_some() {
                return Ok(json!({ "state": "terminal" }));
            }
            if matches!(st.phase, Phase::Running | Phase::Approval) || st.sdk.limit_firing {
                st.sdk.term_after = true;
                return Ok(json!({ "state": "after_turn" }));
            }
            match st.binding.clone().filter(|b| b.transcript_path.is_file()) {
                Some(b) => b,
                None => return Ok(json!({ "state": "no_session" })),
            }
        };
        let pane = self.instances.pane(id).await.ok_or_else(|| LocalError::new("CHAT_UNAVAILABLE"))?;
        if !super::is_shell(&pane.command) {
            return Ok(json!({ "state": "busy" }));
        }
        let line = self.instances.claude_tui_resume_line(id, &b.session_id)?;
        // 和起进程（sdk_attach）错开：正在起的进程还没记进 st.sdk.proc，这时结束不掉它，
        // 结果两个 Claude Code 同时写这段对话、“在终端里继续”一直挂着
        let _g = self.sdk.start.lock().await;
        let now = crate::util::system_ms();
        {
            let mut st = lock(&inst.st);
            if st.sdk.term.is_some() {
                return Ok(json!({ "state": "terminal" }));
            }
            // 等着的时候有人发了消息；或者对话页正在发（起进程、送消息，还没进“在干活”）
            if matches!(st.phase, Phase::Running | Phase::Approval) || st.sdk.sending > 0 {
                st.sdk.term_after = true;
                return Ok(json!({ "state": "after_turn" }));
            }
            st.sdk.term = Some(now);
            st.sdk.term_after = false;
            // 在等额度重置后自动继续的：交给终端就不替它等了
            self.limit_set(&inst, &mut st, None);
            if let Some(p) = st.sdk.proc.take() {
                p.kill();
            }
            Self::guard_turn(&mut st);
            Self::set_phase(&inst, &mut st, Phase::Idle, None);
            Self::emit_status(&inst, &st);
        }
        let _ = self.store.set(&term_key(id), &now);
        drop(_g);
        // 进程刚结束，等它放开记录文件；接着的是对话页显示的那一支（编辑重发分出来的，见 pin_leaf）
        tokio::time::sleep(Duration::from_millis(300)).await;
        if let Err(e) = pin_leaf(&b.transcript_path, &b.session_id) {
            tracing::debug!(error = %e, "交给终端前没能记下要接着的那一支");
        }
        tracing::info!(id, session = %b.session_id, "对话交给终端里的 Claude Code");
        self.instances.send_literal(id, &line).await?;
        self.instances.send_keys(id, &["Enter"]).await?;
        Ok(json!({ "state": "terminal" }))
    }

    /// 从终端收回来：退出终端里的 TUI（空闲时连按两次 Ctrl+C；在跑的第一下是中断）。
    /// 没交给终端时什么都不做
    async fn sdk_take_back(self: &Arc<Self>, inst: &Arc<Inst>) -> Result<(), LocalError> {
        if lock(&inst.st).sdk.term.is_none() {
            return Ok(());
        }
        // 换了对话的已经由 sdk_term_tick 跟过去了；这里不再跟：在对话页 /resume、/new 时绑定的是用户新选的那段
        for _ in 0..3 {
            let Some(pane) = self.instances.pane(&inst.id).await else { break };
            if super::is_shell(&pane.command) {
                break;
            }
            let _ = self.instances.send_keys(&inst.id, &["C-c"]).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = self.instances.send_keys(&inst.id, &["C-c"]).await;
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
        if self.instances.pane(&inst.id).await.is_some_and(|p| !super::is_shell(&p.command)) {
            return Err(LocalError::new("CHAT_TERMINAL_BUSY"));
        }
        tracing::info!(id = %inst.id, "对话从终端收回到对话界面");
        self.sdk_term_clear(inst);
        Ok(())
    }

    fn sdk_term_clear(&self, inst: &Inst) {
        let mut st = lock(&inst.st);
        st.sdk.term = None;
        // 终端里的 TUI 退出了：它挂在对话页的确认框、提问没人能答了
        Self::drop_pending(&mut st);
        let _ = self.store.remove(&term_key(&inst.id));
        // 终端里退出时可能停在“在干活”（中断的那一轮没有 Stop）
        if matches!(st.phase, Phase::Running | Phase::Approval) {
            Self::set_phase(inst, &mut st, Phase::Idle, None);
        } else {
            Self::emit_status(inst, &st);
        }
    }

    /// 终端里的 TUI 换了对话（/clear、/resume）：对话页跟过去（会话列表认出来的对话号，见 session_probe）
    async fn sdk_term_follow(&self, inst: &Arc<Inst>) {
        let Ok(v) = self.instances.get(&inst.id).await else { return };
        let Some(conv) = v.conv_id.filter(|c| crate::history::valid_id(c)) else { return };
        if lock(&inst.st).binding.as_ref().is_some_and(|b| b.session_id == conv) {
            return;
        }
        let Some(path) = claude_sdk::find_transcript(&self.claude_config_dirs(), &conv) else { return };
        tracing::info!(id = %inst.id, conv, "终端里换了对话，对话页跟过去");
        let b = super::Binding { session_id: conv.clone(), transcript_path: path };
        let _ = self.store.set(&super::binding_key(&inst.id), &b);
        crate::history::mark_own(&self.store, &conv);
        {
            let mut st = lock(&inst.st);
            st.binding = Some(b);
            st.title = None;
            st.board_dirty = true;
        }
        *inst.tr.lock().await = None;
        let _ = inst.tx.send(super::Event::Reset);
    }

    /// 后台（每 9 秒）：交给终端的，终端里的 TUI 自己退出了（/exit、Ctrl+C）就收回来；换了对话就跟过去
    pub(super) async fn sdk_term_tick(self: &Arc<Self>, inst: &Arc<Inst>) {
        if lock(&inst.st).sdk.term.is_none() {
            return;
        }
        // 看看起的进程还在：对话页在用（交接撞上了起进程），不算在终端里，也不能跟着终端换对话（/new 会被换回去）
        if lock(&inst.st).sdk.proc.as_ref().is_some_and(|p| p.alive()) {
            tracing::warn!(id = %inst.id, "交给终端了但对话模式的进程还在，按对话页在用算");
            self.sdk_term_clear(inst);
            return;
        }
        // 终端的 tmux 会话没了（tmux 服务重启、会话被关）：终端里的 Claude 也没了
        let Some(pane) = self.instances.pane(&inst.id).await else {
            tracing::info!(id = %inst.id, "交给终端了但终端会话不在了，对话回到对话界面");
            self.sdk_term_clear(inst);
            return;
        };
        self.sdk_term_follow(inst).await;
        // 刚交过去时 shell 还没起 claude：给 15 秒
        let since = lock(&inst.st).sdk.term.unwrap_or(0);
        if super::is_shell(&pane.command) && crate::util::system_ms() - since > 15_000 {
            tracing::info!(id = %inst.id, "终端里的 Claude Code 退出了，对话回到对话界面");
            self.sdk_term_clear(inst);
        }
    }

    /// 客户端启动时：重启前交给了终端的接着算在终端里（终端会话在 tmux 里，客户端重启不受影响）
    pub(super) fn sdk_term_restore(&self) {
        for r in self.store.instances().unwrap_or_default() {
            let key = term_key(&r.id);
            let Ok(Some(at)) = self.store.get::<i64>(&key) else { continue };
            if !self.instances.claude_sdk_mode(&r) {
                let _ = self.store.remove(&key);
                continue;
            }
            let inst = self.inst(&r.id);
            inst.sdk.store(true, std::sync::atomic::Ordering::Relaxed);
            let mut st = lock(&inst.st);
            st.sdk.term = Some(at);
            Self::set_phase(&inst, &mut st, Phase::Idle, None);
        }
    }

    /// 后台：结束空闲很久、没人看、没有后台任务、没有定时任务、不在等额度重置的进程
    /// Claude Code 装了新版本（看看自动更新的、用户自己更新的、Claude Code 自己后台更新的）：对话模式的进程还是旧版本。
    /// 标上，空闲时结束（[`Chat::sdk_upgrade`]），下次发消息 `--resume` 用新版本接着，对话不断。终端里的 TUI 换不了，界面上提示重开
    pub fn claude_installed(&self, version: &str) {
        for inst in lock(&self.insts).values() {
            if !inst.sdk.load(std::sync::atomic::Ordering::Relaxed) {
                continue;
            }
            let mut st = lock(&inst.st);
            if st.sdk.proc.is_some() && st.version.as_deref().is_some_and(|v| crate::agent_update::newer(version, v)) && st.sdk.upgrade.as_deref() != Some(version) && st.sdk.upgraded.as_deref() != Some(version) {
                tracing::info!(id = %inst.id, from = ?st.version, to = version, "Claude Code 有新版本了，这个对话空闲时换上");
                st.sdk.upgrade = Some(version.to_string());
            }
        }
    }

    /// 标了要换新版本、现在空闲（不在跑、后台没有命令和子代理、没有定时任务、没在等额度）：结束旧进程。
    /// 跟空闲回收一样，下次发消息时接着这段对话起新的
    pub(super) fn sdk_upgrade(&self, inst: &Inst) {
        let mut st = lock(&inst.st);
        if st.sdk.upgrade.is_none() || st.sdk.term.is_some() {
            return;
        }
        let idle = st.phase == Phase::Idle && st.background.is_empty() && st.agents.is_empty() && !st.sdk.crons && st.sdk.limit_resume.is_none() && st.sdk.sending == 0 && st.pending.is_none();
        if !idle {
            return;
        }
        let to = st.sdk.upgrade.take();
        st.sdk.upgraded = to.clone();
        if let Some(p) = st.sdk.proc.take() {
            tracing::info!(id = %inst.id, from = ?st.version, to = ?to, "对话空闲，结束旧版本的 Claude Code，下次发消息时用新版本接着");
            p.kill();
        }
    }

    pub(super) fn sdk_reap(&self, inst: &Inst) {
        let mut st = lock(&inst.st);
        let idle = st.phase == Phase::Idle && st.background.is_empty() && st.agents.is_empty() && !st.sdk.crons && st.sdk.limit_resume.is_none() && st.sdk.idle_since.is_some_and(|t| t.elapsed() >= IDLE_REAP);
        if idle && st.sdk.proc.is_some() && inst.viewers.load(std::sync::atomic::Ordering::SeqCst) == 0 {
            tracing::info!(id = %inst.id, "对话模式的 Claude Code 空闲很久了，先结束，用到时再接着");
            if let Some(p) = st.sdk.proc.take() {
                p.kill();
            }
        }
    }
}

/// 额度用完时 claude 给的话：`You've hit your limit · resets 3pm (Asia/Shanghai)`、`Claude AI usage limit reached|1730000000`；
/// 第三方接口转过来的：`API Error: 429 …`、智谱 `1308 已达到 5 小时的使用上限`、MiniMax `usage limit exceeded`、`quota exceeded`
fn is_limit_text(s: &str) -> bool {
    let s = s.to_lowercase();
    s.contains("usage limit reached")
        || (s.contains("hit your") && s.contains("limit"))
        || s.contains("api error: 429")
        || s.contains("usage limit")
        || s.contains("rate limit exceeded")
        || s.contains("quota exceeded")
        || s.contains("exceeded your current quota")
        || ["使用上限", "额度已用完", "用量已达上限", "达到上限", "超出限额", "限额"].iter().any(|k| s.contains(k))
}

/// 第三方接口的套餐到期、余额 / 资源包用完（等重置也没用，要续费或换账号）。
/// 智谱：`1113 余额不足或无可用资源包`、`1309 您的套餐已到期`；Kimi / MiniMax / DeepSeek：`insufficient balance`、`Insufficient Balance`；
/// 一般的 402
fn is_expired_text(s: &str) -> bool {
    let l = s.to_lowercase();
    ["套餐已到期", "套餐已过期", "已过期", "已到期", "余额不足", "无可用资源包", "欠费", "insufficient balance", "insufficient_balance", "insufficient account balance", "plan has expired", "subscription has expired", "subscription expired", "payment required", "api error: 402"]
        .iter()
        .any(|k| l.contains(k))
}

/// 报错原文里写的重置时间（Unix 秒），过去的不算：
/// `Claude AI usage limit reached|1730000000`、`您的限额将在 2025-08-23 21:40:58 重置`、`resets at 2025-08-23T13:40:58Z`。
/// 不带时区的：中文的按北京时间（国内服务商），别的按 UTC
fn reset_in_text(s: &str, now: i64) -> Option<i64> {
    if let Some(t) = s.split('|').nth(1).and_then(|x| x.trim().get(..10)).and_then(|x| x.parse::<i64>().ok()).filter(|&t| t > now) {
        return Some(t);
    }
    let l = s.to_lowercase();
    if !(l.contains("重置") || l.contains("reset") || l.contains("恢复")) {
        return None;
    }
    let b = s.as_bytes();
    let digit = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    for i in 0..b.len() {
        // YYYY-MM-DD[ T]HH:MM[:SS]
        let ok = (0..4).all(|k| digit(i + k)) && b.get(i + 4) == Some(&b'-') && digit(i + 5) && digit(i + 6) && b.get(i + 7) == Some(&b'-') && digit(i + 8) && digit(i + 9)
            && matches!(b.get(i + 10), Some(b' ' | b'T')) && digit(i + 11) && digit(i + 12) && b.get(i + 13) == Some(&b':') && digit(i + 14) && digit(i + 15);
        if !ok || (i > 0 && digit(i - 1)) {
            continue;
        }
        let num = |a: usize, n: usize| s[a..a + n].parse::<i64>().ok();
        let sec = if b.get(i + 16) == Some(&b':') && digit(i + 17) && digit(i + 18) { num(i + 17, 2)? } else { 0 };
        let end = if sec > 0 || b.get(i + 16) == Some(&b':') { i + 19 } else { i + 16 };
        let date = time::Date::from_calendar_date(num(i, 4)? as i32, time::Month::try_from(num(i + 5, 2)? as u8).ok()?, num(i + 8, 2)? as u8).ok()?;
        let tod = time::Time::from_hms(num(i + 11, 2)? as u8, num(i + 14, 2)? as u8, sec as u8).ok()?;
        let dt = time::PrimitiveDateTime::new(date, tod);
        // 后面跟着 Z 或 +08:00 的按它，否则按本机时区
        let rest = s.get(end..).unwrap_or("");
        let off = if rest.starts_with('Z') {
            Some(time::UtcOffset::UTC)
        } else if rest.len() >= 6 && (rest.starts_with('+') || rest.starts_with('-')) && rest.as_bytes()[3] == b':' {
            let sign = if rest.starts_with('-') { -1 } else { 1 };
            let (h, m) = (rest[1..3].parse::<i8>().ok()?, rest[4..6].parse::<i8>().ok()?);
            time::UtcOffset::from_hms(sign * h, sign * m, 0).ok()
        } else {
            None
        };
        // 不带时区：中文的报错是国内服务商写的北京时间，别的按 UTC
        let cjk = s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        let off = off.unwrap_or(if cjk { time::UtcOffset::from_hms(8, 0, 0).ok()? } else { time::UtcOffset::UTC });
        let t = dt.assume_offset(off).unix_timestamp();
        return (t > now).then_some(t);
    }
    None
}

/// 存“交给终端了”（毫秒）的键
fn term_key(id: &str) -> String {
    format!("chat.sdk_term.{id}")
}

/// 存“额度重置后自动继续”时间（毫秒）的键
fn limit_key(id: &str) -> String {
    format!("chat.limit_resume.{id}")
}

/// StopFailure 的 `error`（`rate_limit`、`billing_error`……），不是报错原文
fn is_error_code(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

/// 用量接口（[`Chat::usage`]）里用完的那个窗口什么时候重置（Unix 秒）
fn limit_reset_in(u: &Value) -> Option<i64> {
    let at = |w: &Value| w["resets_at"].as_str().and_then(|s| time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()).map(|t| t.unix_timestamp());
    let l = &u["limits"];
    ["five_hour", "seven_day"].iter().map(|k| &l[*k]).filter(|w| w["used"].as_f64().is_some_and(|x| x >= 100.0)).filter_map(at).max()
}

/// 用量（[`Chat::usage`]）说额度已经回来了：`since`（毫秒）之后取的，有 5 小时或 7 天的数，而且没有一个用满。
/// 过了重置时间的窗口在用量里是 null（[`models::limits`]），也算回来了
fn limit_back(u: &Value, since: i64) -> bool {
    let l = &u["limits"];
    let fresh = l["fetched_at"].as_i64().is_none_or(|t| t >= since);
    let wins = [&l["five_hour"], &l["seven_day"]];
    let known = wins.iter().any(|w| w["used"].is_number());
    let full = wins.iter().any(|w| w["used"].as_f64().is_some_and(|x| x >= 100.0));
    fresh && known && !full
}

/// 工具结果里的后台任务 id：`Command running in background with ID: b14bc2oy7.`
fn task_id_in(content: &str) -> Option<String> {
    let rest = content.split("with ID: ").nth(1)?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    (!id.is_empty()).then_some(id)
}

/// `--resume <会话>` 接着的是 Claude Code 自己记的那一支：transcript 里最后一条 `last-prompt` 的 `leafUuid`
/// （正常退出时才写）。编辑重发分出新的一支后进程被结束（换账号、重启客户端、空闲退出、再编辑一次），
/// 这条还指着改之前的那一支，下次接着时模型又回到改之前的对话，`--resume-session-at` 也找不到新那支上的记录（实测 2.1.319）。
/// 所以起进程前按界面的显示规则（最后写下的一条主线记录，[`super::transcript`]）补一条 `last-prompt`，两边一致。
/// 进程这时还没起，不会和 Claude Code 同时写。
fn pin_leaf(path: &std::path::Path, sid: &str) -> std::io::Result<()> {
    use std::io::{BufRead, Write};
    let f = std::fs::File::open(path)?;
    let (mut leaf, mut pinned, mut prompt): (Option<String>, Option<String>, Option<String>) = (None, None, None);
    for line in std::io::BufReader::new(f).lines() {
        let line = line?;
        // 先粗筛，几十 MB 的 transcript 也不用每行都解析
        if !line.contains("\"uuid\"") && !line.contains("\"last-prompt\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str().unwrap_or("") {
            "last-prompt" => {
                pinned = v["leafUuid"].as_str().map(String::from);
                prompt = v["lastPrompt"].as_str().map(String::from);
            }
            "user" | "assistant" | "attachment" | "system" if v["isSidechain"] != true => {
                if let Some(u) = v["uuid"].as_str() {
                    leaf = Some(u.to_string());
                }
            }
            _ => {}
        }
    }
    let Some(leaf) = leaf.filter(|l| pinned.as_deref() != Some(l.as_str())) else { return Ok(()) };
    let rec = json!({ "type": "last-prompt", "lastPrompt": prompt.unwrap_or_default(), "leafUuid": leaf, "sessionId": sid });
    let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
    f.write_all(format!("{rec}\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_env_like_terminal() {
        let kv = |k: &str, v: &str| (k.to_string(), v.to_string());
        let user = vec![kv("HTTPS_PROXY", "http://127.0.0.1:7890"), kv("PWD", "/x"), kv("TMUX", "/tmp/t"), kv("LOOKLOOK_INSTANCE", "a"), kv("CLAUDE_CODE_ENTRYPOINT", "cli"), kv("CLAUDE_CODE_MAX_OUTPUT_TOKENS", "9000"), kv("PATH", "/bin")];
        let env = sdk_env("/bin/zsh", user);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(get("HTTPS_PROXY"), Some("http://127.0.0.1:7890"));
        assert_eq!(get("CLAUDE_CODE_MAX_OUTPUT_TOKENS"), Some("9000"));
        assert_eq!(get("SHELL"), Some("/bin/zsh"));
        for k in ["PWD", "TMUX", "LOOKLOOK_INSTANCE", "CLAUDE_CODE_ENTRYPOINT", "PATH"] {
            assert_eq!(get(k), None, "{k}");
        }
        if std::env::var_os("CLAUDE_CODE_ARTIFACT").is_none() {
            assert_eq!(get("CLAUDE_CODE_ARTIFACT"), Some("1"));
            // 用户自己关了 Artifact：照他的
            let env = sdk_env("/bin/zsh", vec![kv("CLAUDE_CODE_ARTIFACT", "0")]);
            assert_eq!(env.iter().filter(|(k, _)| k == "CLAUDE_CODE_ARTIFACT").map(|(_, v)| v.as_str()).collect::<Vec<_>>(), vec!["0"]);
        }
    }

    #[test]
    fn account_fast_setting() {
        let dir = std::env::temp_dir().join(format!("ll-fast-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!account_fast(&dir));
        std::fs::write(dir.join("settings.json"), r#"{"fastMode":true}"#).unwrap();
        assert!(account_fast(&dir));
        std::fs::write(dir.join("settings.json"), r#"{"fastMode":false}"#).unwrap();
        assert!(!account_fast(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn limit_and_error_codes() {
        assert!(is_limit_text("You've hit your weekly limit · resets Oct 18, 11pm (America/New_York)"));
        assert!(is_limit_text("Claude AI usage limit reached|1792378800"));
        assert!(!is_limit_text("API Error: 500 Internal server error"));
        assert!(is_limit_text("API Error: 429 {\"error\":{\"code\":\"1308\",\"message\":\"已达到 5 小时的使用上限。您的限额将在 2026-11-10 15:00:00 重置。\"}}"));
        assert!(is_expired_text("API Error: 429 {\"error\":{\"code\":\"1113\",\"message\":\"余额不足或无可用资源包,请充值。\"}}"));
        assert!(!is_expired_text("API Error: 429 已达到 5 小时的使用上限"));
        let now = 1_700_000_000;
        assert_eq!(reset_in_text("Claude AI usage limit reached|1792378800", now), Some(1_792_378_800));
        assert_eq!(reset_in_text("limit resets at 2026-11-10T15:00:00Z", now), Some(1_794_322_800));
        assert_eq!(reset_in_text("您的限额将在 2026-11-10 23:00:00+08:00 重置", now), Some(1_794_322_800));
        assert_eq!(reset_in_text("已达到 5 小时的使用上限。您的限额将在 2026-11-10 23:00:00 重置。", now), Some(1_794_322_800), "北京时间");
        assert_eq!(reset_in_text("您的限额将在 2020-01-01 00:00:00 重置", now), None);
        assert_eq!(reset_in_text("API Error: 429 Too Many Requests", now), None);
        assert!(is_error_code("billing_error") && is_error_code("rate_limit"));
        assert!(!is_error_code("Credit balance is too low") && !is_error_code(""));
        let u = json!({ "limits": {
            "five_hour": { "used": 40.0, "resets_at": "2026-11-07T15:00:00Z" },
            "seven_day": { "used": 100.0, "resets_at": "2026-11-09T03:00:00+00:00" },
        } });
        assert_eq!(limit_reset_in(&u), Some(1_794_193_200));
        assert_eq!(limit_reset_in(&json!({ "limits": { "five_hour": { "used": 10.0, "resets_at": "2026-11-07T15:00:00Z" } } })), None);
    }

    #[test]
    fn limit_back_early() {
        let u = |five: Value, week: Value, at: i64| json!({ "limits": { "five_hour": five, "seven_day": week, "fetched_at": at } });
        let w = |used: f64| json!({ "used": used, "resets_at": "2999-01-01T00:00:00Z" });
        // 周额度用完，在等；周额度提前刷新了（新取的用量里不满了）→ 提前继续
        assert!(limit_back(&u(w(30.0), w(2.0), 200), 100));
        // 5 小时窗口还满着：接着等
        assert!(!limit_back(&u(w(100.0), w(2.0), 200), 100));
        assert!(!limit_back(&u(w(30.0), w(100.0), 200), 100));
        // 撞上限额之前取的旧用量不算
        assert!(!limit_back(&u(w(30.0), w(2.0), 50), 100));
        // 什么数都没有（没取到）不算
        assert!(!limit_back(&u(Value::Null, Value::Null, 200), 100));
        assert!(!limit_back(&json!({ "limits": null }), 100));
        // 过了重置时间的窗口是 null：另一个不满就算回来了
        assert!(limit_back(&u(w(10.0), Value::Null, 200), 100));
    }

    #[test]
    fn task_ids_from_results() {
        assert_eq!(task_id_in("Command running in background with ID: b14bc2oy7. Output is being written to: /tmp/x"), Some("b14bc2oy7".into()));
        assert_eq!(task_id_in("done"), None);
    }

    #[test]
    fn pin_leaf_follows_the_edited_branch() {
        let l = |v: Value| v.to_string() + "\n";
        let path = std::env::temp_dir().join(format!("ll-pin-leaf-{}.jsonl", std::process::id()));
        // u2 → a2 是老的一支（Claude Code 记的 leaf 是 a2），编辑重发后从 a1 分出 u3 → a3，进程没正常退出
        let mut s = l(json!({"type":"user","uuid":"u1","parentUuid":null,"message":{"content":"1"}}));
        s += &l(json!({"type":"assistant","uuid":"a1","parentUuid":"u1","message":{"content":[]}}));
        s += &l(json!({"type":"user","uuid":"u2","parentUuid":"a1","message":{"content":"2"}}));
        s += &l(json!({"type":"assistant","uuid":"a2","parentUuid":"u2","message":{"content":[]}}));
        s += &l(json!({"type":"last-prompt","lastPrompt":"2","leafUuid":"a2","sessionId":"s"}));
        s += &l(json!({"type":"user","uuid":"u3","parentUuid":"a1","message":{"content":"3"}}));
        s += &l(json!({"type":"assistant","uuid":"a3","parentUuid":"u3","message":{"content":[]}}));
        s += &l(json!({"type":"assistant","uuid":"x1","parentUuid":"a3","isSidechain":true,"message":{"content":[]}}));
        s += &l(json!({"type":"cost-state","sessionId":"s"}));
        std::fs::write(&path, &s).unwrap();
        pin_leaf(&path, "s").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let last: Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
        assert_eq!(last["leafUuid"], "a3", "接着界面上显示的那一支，不算子代理的记录");
        assert_eq!(last["lastPrompt"], "2");
        // 已经一致：不再写
        pin_leaf(&path, "s").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        let _ = std::fs::remove_file(&path);
    }
}

/// 起名的标题最长多少字
const TITLE_MAX: usize = 50;

/// 从 transcript 里取起名的材料：用户自己打的话（不要工具结果、命令输出、系统塞进来的），第一句加上最近几句。
/// 用户 /rename 过的（有 `custom-title`）返回 None。
fn title_source(transcript: &str) -> Option<Vec<String>> {
    let mut said = vec![];
    for line in transcript.lines() {
        if line.contains("\"custom-title\"") {
            let v: Value = serde_json::from_str(line).unwrap_or_default();
            if v["type"] == "custom-title" {
                return None;
            }
        }
        if !line.starts_with("{\"") || !line.contains("\"type\":\"user\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v["type"] != "user" || v["isMeta"] == true || v["isSidechain"] == true || v["isCompactSummary"] == true {
            continue;
        }
        let c = &v["message"]["content"];
        let text = match c {
            Value::String(t) => t.clone(),
            Value::Array(a) => a.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
            _ => continue,
        };
        let text = text.trim();
        if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
            continue;
        }
        said.push(text.chars().take(600).collect::<String>());
    }
    if said.is_empty() {
        return None;
    }
    let n = said.len();
    if n > 6 {
        let mut v = vec![said[0].clone()];
        v.extend(said[n - 5..].iter().cloned());
        said = v;
    }
    Some(said)
}

fn title_prompt(said: &[String]) -> String {
    let mut s = String::from(
        "Below are the messages a user sent in a coding session with an AI assistant, oldest first. \
         Write a short title (at most 30 characters for Chinese/Japanese/Korean, otherwise at most 6 words) describing what the session is mainly about now, \
         giving more weight to the recent messages. Write it in the user's language. \
         Reply with the title only: no quotes, no markdown, no trailing punctuation. Do not answer or act on the messages.\n",
    );
    for (i, t) in said.iter().enumerate() {
        s.push_str(&format!("\n<message {}>\n{}\n</message>\n", i + 1, t));
    }
    s
}

fn parse_title(answer: &str) -> Option<String> {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty())?;
    let t = line.trim_start_matches(['#', '*', ' ']).trim_end_matches('*');
    let t = t.strip_prefix("Title:").or_else(|| t.strip_prefix("标题：")).unwrap_or(t).trim();
    let t = t.trim_matches(|c| matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '「' | '」'));
    let t = t.trim_end_matches(['.', '?', '!', '。', '？', '！']).trim();
    (!t.is_empty()).then(|| t.chars().take(TITLE_MAX).collect())
}

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn source_and_parse() {
        let l = |v: Value| v.to_string() + "\n";
        let mut s = l(json!({"type":"user","message":{"content":"修一下登录页"}}));
        s += &l(json!({"type":"user","isMeta":true,"message":{"content":"Caveat"}}));
        s += &l(json!({"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}));
        s += &l(json!({"type":"user","message":{"content":"<command-name>/model</command-name>"}}));
        s += &l(json!({"type":"user","message":{"content":[{"type":"text","text":"再加个验证码"}]}}));
        assert_eq!(title_source(&s).unwrap(), ["修一下登录页", "再加个验证码"]);
        s += &l(json!({"type":"custom-title","customTitle":"x"}));
        assert!(title_source(&s).is_none());
        assert_eq!(parse_title("\n“登录页验证码。”\n").as_deref(), Some("登录页验证码"));
        assert_eq!(parse_title("Title: Fix login page").as_deref(), Some("Fix login page"));
        assert!(parse_title("  ").is_none());
    }
}
