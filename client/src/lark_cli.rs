//! 飞书 CLI（lark-cli，飞书官方）在这台电脑上到哪一步了、下一步怎么走（docs/FEISHU_BOT.md §4）。
//!
//! 没装 → 装（和 AI 页同一个安装任务）→ `config init --new`（建 CLI 自己用的应用，给一个链接）
//! → `auth login --recommend --no-wait --json`（用你的飞书账号授权，给一个链接，后台 `--device-code` 等结果）→ 好了。
//! 链接都是飞书的授权页，手机飞书里点开就能做完。飞书机器人的卡片和助理页都用这里。

use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::error::LocalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// 没装
    Missing,
    /// 装了，还没配应用
    NeedConfig,
    /// 配了应用，还没用你的账号登录
    NeedLogin,
    Ready,
}

#[derive(Debug, Clone, Serialize)]
pub struct Status {
    pub state: State,
    /// 正在等你点的链接（第 1 步或第 2 步）
    pub url: Option<String>,
    /// 正在进行的一步：install / config / login
    pub running: Option<&'static str>,
    /// 上一步失败的原因（输出的最后几行）
    pub error: Option<String>,
    /// 装需要 npm
    pub npm: bool,
}

/// 正在进行的一步
struct Step {
    kind: &'static str,
    url: Option<String>,
    started: Instant,
    /// 结束了：Ok / 失败原因
    done: Option<Result<(), String>>,
}

static STEP: Mutex<Option<Step>> = Mutex::new(None);
/// 找到的 lark-cli 的完整路径（`tools::detect` 用的是交互式登录 shell，读 `.zshrc`；跑命令的 `-lc` 不读，
/// npm / nvm 的 PATH 写在 `.zshrc` 里时直接写 `lark-cli` 会 command not found）
static BIN: Mutex<Option<String>> = Mutex::new(None);
/// 状态缓存（每句飞书消息都要看一眼，别每次都起进程）
static CACHE: Mutex<Option<(Instant, State, bool)>> = Mutex::new(None);
const CACHE_FOR: Duration = Duration::from_secs(60);
/// 链接等人点的最长时间
const WAIT_USER: Duration = Duration::from_secs(15 * 60);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 单引号包起来给 sh / zsh
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `lark-cli …` → 用找到的完整路径跑；PATH 换成用户平时终端里的（它是 `#!/usr/bin/env node` 的脚本，要找得到 node），
/// 它所在的目录放最前面
fn resolve(line: &str, bin: Option<&str>, user_path: Option<&str>) -> String {
    if cfg!(windows) {
        return line.to_string();
    }
    let mut path: Vec<String> = bin.and_then(|b| std::path::Path::new(b).parent()).map(|d| d.display().to_string()).into_iter().collect();
    path.extend(user_path.map(str::to_string));
    let line = match (line.strip_prefix("lark-cli "), bin) {
        (Some(rest), Some(bin)) => format!("{} {rest}", quote(bin)),
        _ => line.to_string(),
    };
    if path.is_empty() {
        return line;
    }
    format!("PATH={}:\"$PATH\" {line}", quote(&path.join(":")))
}

async fn command(shell: &str, line: &str) -> Command {
    let user_path = crate::chat::codex_app::user_path(shell).await;
    let bin = lock(&BIN).clone();
    let line = &resolve(line, bin.as_deref(), user_path.as_deref());
    if cfg!(windows) {
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut c = Command::new("cmd.exe");
        #[cfg(windows)]
        {
            c.args(["/D", "/S", "/C"]).raw_arg(format!("\"{line}\""));
            c.creation_flags(0x0800_0000);
        }
        c
    } else {
        let mut c = Command::new(shell);
        c.args(["-lc", line]);
        c
    }
}

/// 跑一条，等它结束：(成功没有, stdout, stderr)
async fn run(shell: &str, line: &str, wait: Duration) -> Option<(bool, String, String)> {
    let mut c = command(shell, line).await;
    c.stdin(Stdio::null()).env("NO_COLOR", "1").kill_on_drop(true);
    let out = tokio::time::timeout(wait, c.output()).await.ok()?.ok()?;
    Some((out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
}

/// 输出里的第一个 JSON 对象
fn json_of(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    serde_json::from_str(s.get(start..=end)?).ok()
}

/// 递归找一个非空的字符串字段
fn find_str<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    match v {
        Value::Object(m) => {
            for k in keys {
                if let Some(s) = m.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    return Some(s);
                }
            }
            m.values().find_map(|x| find_str(x, keys))
        }
        Value::Array(a) => a.iter().find_map(|x| find_str(x, keys)),
        _ => None,
    }
}

/// `auth status --json` 的输出 → 状态（1.0.100 实测：没配 = `error.subtype = not_configured`）
fn parse_status(out: &str) -> State {
    let Some(v) = json_of(out) else {
        // 找不到命令（不该发生：用的是检测到的完整路径）：当没装，别显示成“建应用”那一步
        return if out.contains("command not found") || out.contains("No such file") { State::Missing } else { State::NeedConfig };
    };
    if find_str(&v, &["subtype", "type"]).is_some_and(|s| s == "not_configured") {
        return State::NeedConfig;
    }
    let user = find_str(&v, &["userOpenId", "user_open_id"]).is_some();
    let expired = find_str(&v, &["tokenStatus"]).is_some_and(|s| matches!(s, "expired" | "invalid" | "missing" | "none"));
    if user && !expired {
        State::Ready
    } else {
        State::NeedLogin
    }
}

/// 输出里的第一个 https 链接
fn url_in(s: &str) -> Option<String> {
    let i = s.find("https://")?;
    let u: String = s[i..].chars().take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | ')' | ']')).collect();
    (u.len() > 10).then_some(u)
}

/// 现在的状态（`fresh`：不用缓存）
pub async fn status(shell: &str, fresh: bool) -> Status {
    let cached = lock(&CACHE).clone();
    let (state, npm) = match cached {
        Some((at, s, npm)) if !fresh && at.elapsed() < CACHE_FOR => (s, npm),
        _ => {
            let tools = crate::tools::detect(shell).await;
            *lock(&BIN) = tools.lark.clone();
            let s = if tools.lark.is_none() {
                State::Missing
            } else {
                match run(shell, "lark-cli auth status --json", Duration::from_secs(10)).await {
                    Some((_, out, err)) => parse_status(&format!("{out}\n{err}")),
                    None => State::NeedConfig,
                }
            };
            let npm = tools.npm.is_some();
            *lock(&CACHE) = Some((Instant::now(), s, npm));
            (s, npm)
        }
    };
    let mut st = Status { state, url: None, running: None, error: None, npm };
    if crate::tools::job("lark-cli").is_some_and(|j| j.state == "running") {
        st.running = Some("install");
    }
    if let Some(s) = lock(&STEP).as_ref() {
        match &s.done {
            None if s.started.elapsed() < WAIT_USER + Duration::from_secs(60) => {
                st.running = Some(s.kind);
                st.url = s.url.clone();
            }
            Some(Err(e)) => st.error = Some(e.clone()),
            _ => {}
        }
    }
    st
}

/// 短状态（给助理每轮的来源行）：好了没有
pub fn ready_cached() -> Option<bool> {
    lock(&CACHE).as_ref().map(|(_, s, _)| *s == State::Ready)
}

fn forget() {
    *lock(&CACHE) = None;
}

/// 走下一步。返回走完开头之后的状态（有链接时带着链接）。`done` 在这一步结束（成功或失败）时调一次
pub async fn next(shell: &str, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Result<Status, LocalError> {
    let st = status(shell, true).await;
    if st.running.is_some() {
        return Ok(st);
    }
    match st.state {
        State::Ready => Ok(st),
        State::Missing => {
            if !st.npm {
                return Err(LocalError::new("LARK_NEED_NODE"));
            }
            crate::tools::start_install(shell, "lark-cli", "npmmirror").ok_or_else(|| LocalError::new("INTERNAL"))?;
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    match crate::tools::job("lark-cli") {
                        Some(j) if j.state == "running" => continue,
                        Some(j) if j.state == "ok" => {
                            forget();
                            return done(Ok(()));
                        }
                        Some(j) => {
                            forget();
                            return done(Err(tail(&j.output)));
                        }
                        None => return done(Err("install job lost".into())),
                    }
                }
            });
            Ok(status(shell, false).await)
        }
        State::NeedConfig => start_step(shell, "config", "lark-cli config init --new --lang zh", None, done).await,
        State::NeedLogin => {
            let (_, out, err) = run(shell, "lark-cli auth login --recommend --no-wait --json", Duration::from_secs(30)).await.ok_or_else(|| LocalError::new("LARK_FAILED"))?;
            let all = format!("{out}\n{err}");
            let v = json_of(&all).unwrap_or_default();
            let url = find_str(&v, &["verification_uri_complete", "verification_url", "verification_uri"]).map(String::from).or_else(|| url_in(&all));
            let code = find_str(&v, &["device_code"]).map(String::from);
            let (Some(url), Some(code)) = (url, code) else {
                tracing::warn!(out = %tail(&all), "飞书 CLI：登录没拿到链接");
                return Err(LocalError::new("LARK_FAILED"));
            };
            if !code.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
                return Err(LocalError::new("LARK_FAILED"));
            }
            start_step(shell, "login", &format!("lark-cli auth login --device-code {code}"), Some(url), done).await
        }
    }
}

fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join("\n").chars().take(500).collect()
}

/// 在后台跑一步（阻塞到用户点完链接的那种命令）。`url` 已知时直接用，不知道时从输出里找（最多等 30 秒）
async fn start_step(shell: &str, kind: &'static str, line: &str, url: Option<String>, done: impl FnOnce(Result<(), String>) + Send + 'static) -> Result<Status, LocalError> {
    let mut c = command(shell, &format!("{line} 2>&1")).await;
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).env("NO_COLOR", "1").kill_on_drop(true);
    let mut child = c.spawn().map_err(|e| LocalError::from(anyhow::Error::from(e)))?;
    *lock(&STEP) = Some(Step { kind, url: url.clone(), started: Instant::now(), done: None });
    let out = child.stdout.take().expect("piped");
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    tokio::spawn(async move {
        let mut tx = Some(tx);
        let mut lines = BufReader::new(out).lines();
        let mut all = String::new();
        let read = async {
            while let Ok(Some(l)) = lines.next_line().await {
                all.push_str(&l);
                all.push('\n');
                if let Some(u) = url_in(&l) {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(Some(u.clone()));
                    }
                    if let Some(s) = lock(&STEP).as_mut().filter(|s| s.kind == kind && s.url.is_none()) {
                        s.url = Some(u);
                    }
                }
            }
            child.wait().await
        };
        let r = match tokio::time::timeout(WAIT_USER, read).await {
            Ok(Ok(st)) if st.success() => Ok(()),
            Ok(Ok(_)) => Err(tail(&all)),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("timeout".into()),
        };
        if let Some(tx) = tx.take() {
            let _ = tx.send(None);
        }
        if let Some(s) = lock(&STEP).as_mut().filter(|s| s.kind == kind) {
            s.done = Some(r.clone());
        }
        forget();
        tracing::info!(kind, ok = r.is_ok(), "飞书 CLI：一步结束");
        done(r);
    });
    if url.is_none() {
        let got = tokio::time::timeout(Duration::from_secs(30), rx).await.ok().and_then(Result::ok).flatten();
        if got.is_none() {
            return Err(LocalError::new("LARK_FAILED"));
        }
    }
    Ok(status(shell, false).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        // 1.0.100 没配时的真实输出
        let not = r#"{ "ok": false, "error": { "type": "config", "subtype": "not_configured", "message": "not configured" } }"#;
        assert_eq!(parse_status(not), State::NeedConfig);
        assert_eq!(parse_status(r#"{ "ok": true, "identity": "bot", "appId": "cli_x" }"#), State::NeedLogin);
        assert_eq!(parse_status(r#"{ "ok": true, "data": { "identity": "user", "userOpenId": "ou_1", "tokenStatus": "valid" } }"#), State::Ready);
        assert_eq!(parse_status(r#"{ "ok": true, "userOpenId": "ou_1", "tokenStatus": "expired" }"#), State::NeedLogin);
        assert_eq!(url_in("Open https://open.feishu.cn/page/cli?code=ab12 to continue"), Some("https://open.feishu.cn/page/cli?code=ab12".into()));
        assert_eq!(url_in("no link"), None);
        assert_eq!(tail("a\n\nb\nc\nd\ne\n"), "b\nc\nd\ne");
        assert_eq!(parse_status("zsh:1: command not found: lark-cli"), State::Missing);
    }

    #[test]
    fn resolve_path() {
        // .zshrc 里加的 npm 目录：用完整路径跑，PATH 带上它的目录和用户终端里的 PATH
        let l = resolve("lark-cli auth status --json", Some("/Users/a/.npm-global/bin/lark-cli"), Some("/opt/homebrew/bin:/usr/bin"));
        assert_eq!(l, "PATH='/Users/a/.npm-global/bin:/opt/homebrew/bin:/usr/bin':\"$PATH\" '/Users/a/.npm-global/bin/lark-cli' auth status --json");
        assert_eq!(resolve("lark-cli x", Some("/it's/lark-cli"), None), "PATH='/it'\\''s':\"$PATH\" '/it'\\''s/lark-cli' x");
        assert_eq!(resolve("lark-cli x", None, None), "lark-cli x");
    }
}
