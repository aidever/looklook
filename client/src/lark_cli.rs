//! 飞书 CLI（lark-cli，飞书官方）：看看只管**装没装**和**引导装**（docs/FEISHU_BOT.md §3）。
//!
//! 装用官方的方式：`npx @larksuite/cli@latest install`（中英文 README 一样；没有终端时它只做两件事——
//! `npm install -g @larksuite/cli` 和 `npx skills add larksuite/cli -y -g`，语言用 `--lang zh|en`）。
//! 装好以后怎么配应用、登录、读写飞书，都由助理照官方配套的 `lark-*` 技能（`lark-shared` 管配置和登录）自己做，看看不插手。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::agents::Agent;
use crate::error::LocalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// 命令或官方技能缺一样
    Missing,
    /// 命令和技能都在（配没配、登没登录由助理照技能处理）
    Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub state: State,
    /// 找得到 `lark-cli` 命令
    pub cli: bool,
    /// 官方技能装了（`lark-shared` 在任意一个 AI 工具的技能目录里）
    pub skills: bool,
    /// 正在装
    pub running: Option<&'static str>,
    /// 上次装失败的原因（输出的最后几行）
    pub error: Option<String>,
    /// 装需要 npm
    pub npm: bool,
    /// 官方的安装命令（界面上给用户看、自己在终端里跑也行）
    pub command: String,
}

/// 状态缓存（每句飞书消息都要看一眼，别每次都起进程）：(时间, 命令在, 技能在, npm 在)
static CACHE: Mutex<Option<(Instant, bool, bool, bool)>> = Mutex::new(None);
const CACHE_FOR: Duration = Duration::from_secs(60);
/// 后台正在重新查（只起一个）
static REFRESHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 技能目录里认这个：官方技能都依赖它（配置、登录、身份）
const MARK_SKILL: &str = "lark-shared";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 官方技能装在哪些目录（`skills add -g` 放进 `~/.agents/skills`，再链接到 Claude Code 的 `skills/`）
fn skill_dirs() -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    vec![home.join(".agents").join("skills"), Agent::Claude.dir().join("skills"), Agent::Codex.dir().join("skills")]
}

fn skills_installed(dirs: &[PathBuf]) -> bool {
    dirs.iter().any(|d| d.join(MARK_SKILL).join("SKILL.md").is_file())
}

/// 官方安装命令：(给人看的, 真正跑的)。中文环境用 npmmirror（官方安装脚本里下载程序本身也会退到它），英文用 npm 官方源；
/// 源用环境变量传，安装脚本里再起的 `npm install -g`、`npx skills add` 也照这个源
pub fn install_command(zh: bool) -> (String, String) {
    let lang = if zh { "zh" } else { "en" };
    let shown = format!("npx @larksuite/cli@latest install --lang {lang}");
    let registry = crate::tools::registry(if zh { "npmmirror" } else { "official" });
    let line = if cfg!(windows) {
        format!("set \"npm_config_registry={registry}\" && npx -y @larksuite/cli@latest install --lang {lang}")
    } else {
        format!("npm_config_registry={registry} npx -y @larksuite/cli@latest install --lang {lang}")
    };
    (shown, line)
}

/// 现在的状态（`fresh`：不用缓存）。不要新的时候缓存过期了也先用旧的，后台再查一次：
/// 查要起一个交互式登录 shell（读 `.zshrc`，装了 oh-my-zsh、nvm 的要一两秒），不能让打开助理页、每句飞书消息等着
pub async fn status(shell: &str, zh: bool, fresh: bool) -> Status {
    let cached = *lock(&CACHE);
    let (cli, skills, npm) = match cached {
        Some((at, c, s, n)) if !fresh => {
            if at.elapsed() >= CACHE_FOR && !REFRESHING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                let shell = shell.to_string();
                tokio::spawn(async move {
                    check(&shell).await;
                    REFRESHING.store(false, std::sync::atomic::Ordering::SeqCst);
                });
            }
            (c, s, n)
        }
        _ => check(shell).await,
    };
    let mut st = Status { state: if cli && skills { State::Ready } else { State::Missing }, cli, skills, running: None, error: None, npm, command: install_command(zh).0 };
    match crate::tools::job("lark-cli") {
        Some(j) if j.state == "running" => st.running = Some("install"),
        Some(j) if j.state == "failed" && st.state == State::Missing => st.error = Some(tail(&j.output)),
        _ => {}
    }
    st
}

/// 查一次（命令、技能、npm）并记进缓存
async fn check(shell: &str) -> (bool, bool, bool) {
    let tools = crate::tools::detect(shell).await;
    let skills = tokio::task::spawn_blocking(|| skills_installed(&skill_dirs())).await.unwrap_or(false);
    let v = (tools.lark.is_some(), skills, tools.npm.is_some());
    *lock(&CACHE) = Some((Instant::now(), v.0, v.1, v.2));
    v
}

/// 短状态（给助理每轮的来源行）：装好了没有
pub fn ready_cached() -> Option<bool> {
    lock(&CACHE).as_ref().map(|(_, c, s, _)| *c && *s)
}

fn forget() {
    *lock(&CACHE) = None;
}

/// 用官方方式装（已经在装就不再起）。`done` 在装完（成功或失败）时调一次
pub async fn install(shell: &str, zh: bool, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Result<Status, LocalError> {
    let st = status(shell, zh, true).await;
    if st.running.is_some() || st.state == State::Ready {
        return Ok(st);
    }
    if !st.npm {
        return Err(LocalError::new("LARK_NEED_NODE"));
    }
    let (shown, line) = install_command(zh);
    // 跑命令的 `-lc` 不读 `.zshrc`：npm / nvm 的目录常只写在那里，换成用户终端里的 PATH
    let line = match crate::chat::codex_app::user_path(shell).await.filter(|_| !cfg!(windows)) {
        Some(p) => format!("PATH={}:\"$PATH\" {line}", quote(&p)),
        None => line,
    };
    crate::tools::start_command(shell, "lark-cli", &shown, &line);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            match crate::tools::job("lark-cli") {
                Some(j) if j.state == "running" => continue,
                Some(j) => {
                    forget();
                    tracing::info!(ok = j.state == "ok", "飞书 CLI：装完了");
                    return done(if j.state == "ok" { Ok(()) } else { Err(tail(&j.output)) });
                }
                None => return done(Err("install job lost".into())),
            }
        }
    });
    Ok(status(shell, zh, false).await)
}

/// 单引号包起来给 sh / zsh
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 输出的最后几行。官方安装脚本没有终端时也画转圈（`\x1b[1G\x1b[J` 清行重画），一行里只留最后一帧、去掉控制符
fn tail(s: &str) -> String {
    let clean: Vec<String> = s.lines().map(|l| strip_ansi(l.rsplit("\x1b[J").next().unwrap_or(l)).trim().to_string()).collect();
    let lines: Vec<&str> = clean.iter().map(String::as_str).filter(|l| !l.is_empty() && !l.starts_with("npm notice")).collect();
    lines[lines.len().saturating_sub(4)..].join("\n").chars().take(500).collect()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                // 参数和中间字符，到 @–~ 结束
                for d in it.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
        } else if c != '\r' {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_and_skills() {
        let (shown, line) = install_command(true);
        assert_eq!(shown, "npx @larksuite/cli@latest install --lang zh");
        if !cfg!(windows) {
            assert_eq!(line, "npm_config_registry=https://registry.npmmirror.com/ npx -y @larksuite/cli@latest install --lang zh");
            assert_eq!(install_command(false).1, "npm_config_registry=https://registry.npmjs.org/ npx -y @larksuite/cli@latest install --lang en");
        }
        let d = tempfile::tempdir().unwrap();
        let dirs = vec![d.path().join("a"), d.path().join("b")];
        assert!(!skills_installed(&dirs));
        std::fs::create_dir_all(d.path().join("b/lark-shared")).unwrap();
        assert!(!skills_installed(&dirs));
        std::fs::write(d.path().join("b/lark-shared/SKILL.md"), "# x").unwrap();
        assert!(skills_installed(&dirs));
        assert_eq!(quote("/it's"), "'/it'\\''s'");
        assert_eq!(tail("a\n\nb\nc\nd\ne\n"), "b\nc\nd\ne");
        let spin = "\x1b[?25l│\n◒  正在安装 @larksuite/cli\x1b[1G\x1b[J◐  正在安装 @larksuite/cli\x1b[1G\x1b[J■  全局安装失败。运行以下命令重试: npm install -g @larksuite/cli\nnpm notice x\n";
        assert_eq!(tail(spin), "│\n■  全局安装失败。运行以下命令重试: npm install -g @larksuite/cli");
    }
}
