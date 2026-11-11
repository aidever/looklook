//! AI 编程工具（Claude Code、Codex、DSH）的新版本检查和更新：AI 助手页“AI 工具版本”卡片（Ai.jsx）。
//!
//! - **查新版本**：后台每 [`CHECK_EVERY`] 查一次 npm 上的最新版（`latest` 标签；npmmirror 连不上再试官方源），
//!   界面上也能点“检查更新”立刻查。三个工具都发在 npm 上，别的安装方式（安装脚本、Homebrew）也拿它当新版本号。
//! - **按用户实际的安装方式更新**（[`locate`] 看程序装在哪）：
//!   - npm / pnpm / bun 全局装的：同一个包管理器装到查到的那个版本，用查到版本的那个源（国内连得上）；
//!   - Claude Code 官方安装脚本装的（`~/.local/share/claude/versions/<版本>`）、认不出来的 Claude Code：`claude update`；
//!   - Homebrew：`brew upgrade [--cask] <名字>`；
//!   - Claude 桌面版自带的 Claude Code：桌面版自己更新，这里不动；认不出的 Codex、DSH：提示用户按自己的方式更新。
//! - **自动更新**：每个工具一个开关（默认关，存在本机，不同步——各台电脑装法不一样）。开着的、有新版本时，
//!   等到没有会话正在用它（AI 在回复、等你确认、后台有任务；Windows 上只要有它的进程在运行就等，文件被占用换不掉）再装，
//!   不打断在跑的会话。已经打开、停着的会话不受影响，下次启动才用新版本（Codex 对话模式的 app-server 空闲时自己换，
//!   DSH 的网页服务没在忙时重启一下）。同一个版本更新失败的，[`RETRY_MS`] 后再试；装完版本没变的（渠道还没有这个版本）不再试。
//! - 安装任务和“一键安装”共用 tools.rs 的后台任务（同一个工具同时只跑一个），进度看 `GET /tools/install/{tool}`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::chat::Chat;
use crate::error::LocalError;
use crate::instances::Instances;
use crate::store::Store;

pub const AGENTS: [&str; 3] = ["claude", "codex", "dsh"];
/// 多久查一次新版本
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);
/// 自动更新多久看一次（有没有新版本、工具空不空闲）
const TICK: Duration = Duration::from_secs(5 * 60);
/// 启动后多久开始第一次（别和启动时的其他事挤在一起）
const FIRST: Duration = Duration::from_secs(90);
/// 同一个版本自动更新失败后，隔多久再试
const RETRY_MS: i64 = 24 * 3600 * 1000;
const KEY: &str = "agent_update";

/// 存在本机数据库里的（kv：[`KEY`]）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Saved {
    /// 开了自动更新的工具
    #[serde(default)]
    auto: BTreeMap<String, bool>,
    /// 查到的最新版本
    #[serde(default)]
    latest: BTreeMap<String, Latest>,
    /// 上次查新版本的时间（毫秒）
    #[serde(default)]
    checked_at: Option<i64>,
    /// 每个工具最近一次更新的结果
    #[serde(default)]
    last: BTreeMap<String, Outcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Latest {
    version: String,
    /// 查到它的源（tools.rs 的镜像名）：装的时候用同一个
    mirror: String,
}

/// 一次更新的结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    /// 要装的版本
    pub target: String,
    pub from: Option<String>,
    /// 装完后的版本
    pub to: Option<String>,
    pub ok: bool,
    /// 失败的原因（界面翻译）：`perm` | `busy` | `network` | `disk` | `node` | `unchanged` | `failed`
    pub error: Option<String>,
    pub auto: bool,
    /// 毫秒
    pub at: i64,
}

static LOCK: Mutex<()> = Mutex::new(());

fn load(store: &Store) -> Saved {
    store.get(KEY).ok().flatten().unwrap_or_default()
}

/// 读改写一次（同一时间只有一个在改）
fn edit<R>(store: &Store, f: impl FnOnce(&mut Saved) -> R) -> Result<R, LocalError> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(store);
    let r = f(&mut s);
    store.set(KEY, &s)?;
    Ok(r)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

// ---------------- 装在哪、怎么装的 ----------------

/// 一个工具在这台电脑上的样子
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Installed {
    pub path: String,
    pub version: Option<String>,
    /// `npm` | `pnpm` | `bun` | `brew` | `native`（Claude Code 官方安装脚本）| `desktop`（Claude 桌面版自带）| `unknown`
    pub method: &'static str,
    /// Homebrew 的包名、是不是 cask
    #[serde(skip)]
    brew: Option<(String, bool)>,
}

/// 路径统一成小写、正斜杠，方便找特征目录
fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_lowercase()
}

/// 从找到的程序（PATH 里的，可能是链接或 Windows 的 `.cmd` 包装）认出安装方式；版本号能从文件读出来的顺便读出来
fn locate(tool: &str, program: &Path) -> Installed {
    let real = std::fs::canonicalize(program).unwrap_or_else(|_| program.to_path_buf());
    let s = norm(&real);
    let mut out = Installed { path: program.display().to_string(), version: None, method: "unknown", brew: None };
    if tool == "claude" && crate::agents::claude_desktop_bundled().is_some_and(|d| norm(&d) == s || norm(&d) == norm(program)) {
        out.method = "desktop";
        return out;
    }
    // Homebrew：…/Caskroom/<名字>/<版本>/… 或 …/Cellar/<名字>/<版本>/…
    for (dir, cask) in [("/caskroom/", true), ("/cellar/", false)] {
        if let Some(i) = s.find(dir) {
            let mut rest = s[i + dir.len()..].split('/');
            if let Some(name) = rest.next().filter(|n| !n.is_empty()) {
                out.method = "brew";
                out.brew = Some((name.to_string(), cask));
                out.version = rest.next().filter(|v| looks_like_version(v)).map(String::from);
                return out;
            }
        }
    }
    // Claude Code 官方安装脚本：~/.local/bin/claude → ~/.local/share/claude/versions/<版本>
    if tool == "claude" {
        if let Some(i) = s.find("/claude/versions/") {
            out.method = "native";
            out.version = s[i + "/claude/versions/".len()..].split('/').next().filter(|v| looks_like_version(v)).map(String::from);
            return out;
        }
        // Windows 上安装脚本直接放 ~/.local/bin/claude.exe（不是链接）
        if s.ends_with("/.local/bin/claude.exe") && dirs::home_dir().is_some_and(|h| h.join(".local/share/claude").is_dir()) {
            out.method = "native";
            return out;
        }
    }
    let Some(pkg) = crate::tools::package(tool) else { return out };
    if let Some(dir) = package_dir(&real, program, pkg) {
        let s = norm(&dir);
        out.method = if s.contains("/.bun/") {
            "bun"
        } else if s.contains("/pnpm/") || s.contains("/.pnpm/") {
            "pnpm"
        } else {
            "npm"
        };
        out.version = std::fs::read_to_string(dir.join("package.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|j| j["version"].as_str().map(String::from));
        return out;
    }
    // pnpm 的全局命令是脚本，不是链接，找不到包目录
    if s.contains("/pnpm/") {
        out.method = "pnpm";
    }
    out
}

/// 程序所在的 npm 包目录（`…/node_modules/<包名>`）：程序在包里面（链接解开以后），
/// 或者旁边就是 `node_modules`（Windows 上 npm 放在全局目录的 `.cmd` 包装）
fn package_dir(real: &Path, program: &Path, pkg: &str) -> Option<PathBuf> {
    let tail = format!("node_modules/{pkg}");
    if let Some(d) = real.ancestors().find(|d| norm(d).ends_with(&tail) && d.join("package.json").is_file()) {
        return Some(d.to_path_buf());
    }
    [program.parent(), real.parent()].into_iter().flatten().map(|d| d.join("node_modules").join(pkg)).find(|d| d.join("package.json").is_file())
}

fn looks_like_version(v: &str) -> bool {
    v.trim_start_matches('v').split(['.', '-', '+']).next().is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) && v.contains('.')
}

/// 读不出来时问程序自己：`claude --version` → `2.1.3 (Claude Code)`，`codex --version` → `codex-cli 0.164.0`
async fn ask_version(shell: &str, program: &str) -> Option<String> {
    let out = crate::agents::cli_output(shell, program, &["--version"], &[], Duration::from_secs(15)).await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().map(|w| w.trim_start_matches('v')).find(|w| looks_like_version(w)).map(String::from)
}

/// 三个工具现在装没装、装在哪、什么版本
pub async fn installed(shell: &str) -> BTreeMap<&'static str, Option<Installed>> {
    let mut d = crate::tools::detect(shell).await;
    if d.claude.is_none() {
        d.claude = crate::agents::claude_fallback();
    }
    let mut out = BTreeMap::new();
    for tool in AGENTS {
        let path = match tool {
            "claude" => d.claude.clone(),
            "codex" => d.codex.clone(),
            _ => d.dsh.clone(),
        };
        let found = match path {
            Some(p) => {
                let mut i = locate(tool, Path::new(&p));
                if i.version.is_none() {
                    i.version = ask_version(shell, &p).await;
                }
                Some(i)
            }
            None => None,
        };
        out.insert(tool, found);
    }
    out
}

/// 更新用的命令：`(界面上显示的, 交给 shell 跑的)`；这种装法不能由看看更新时 None
fn update_command(tool: &str, i: &Installed, target: &str, mirror: &str) -> Option<(String, String)> {
    let quoted = if cfg!(windows) { format!("\"{}\"", i.path) } else { format!("'{}'", i.path.replace('\'', r"'\''")) };
    let pkg = crate::tools::package(tool)?;
    let reg = crate::tools::registry(mirror);
    let shown = match i.method {
        "npm" => format!("npm install -g {pkg}@{target} --registry={reg}"),
        "pnpm" => format!("pnpm add -g {pkg}@{target} --registry={reg}"),
        "bun" => format!("bun add -g {pkg}@{target} --registry={reg}"),
        "brew" => {
            let (name, cask) = i.brew.as_ref()?;
            format!("brew upgrade {}{name}", if *cask { "--cask " } else { "" })
        }
        "native" => "claude update".to_string(),
        "unknown" if tool == "claude" => "claude update".to_string(),
        _ => return None,
    };
    let line = match i.method {
        "npm" => format!("{shown} --loglevel=http --no-fund --no-audit"),
        "native" | "unknown" => format!("{quoted} update"),
        "brew" => format!("HOMEBREW_NO_INSTALL_CLEANUP=1 {shown}"),
        _ => shown.clone(),
    };
    Some((shown, line))
}

// ---------------- 新版本 ----------------

/// `a` 比 `b` 新：按 semver 比（`1.2.3` > `1.2.3-rc.1` > `1.2.3-alpha.3`）
pub fn newer(a: &str, b: &str) -> bool {
    cmp_version(a, b) == Some(std::cmp::Ordering::Greater)
}

fn cmp_version(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    fn parse(v: &str) -> Option<(Vec<u64>, Option<Vec<String>>)> {
        let v = v.trim().trim_start_matches('v');
        let v = v.split('+').next()?;
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p.split('.').map(String::from).collect())),
            None => (v, None),
        };
        let core: Option<Vec<u64>> = core.split('.').map(|p| p.parse().ok()).collect();
        Some((core?, pre))
    }
    let (ca, pa) = parse(a)?;
    let (cb, pb) = parse(b)?;
    let len = ca.len().max(cb.len());
    let pad = |c: &[u64]| (0..len).map(|i| c.get(i).copied().unwrap_or(0)).collect::<Vec<_>>();
    match pad(&ca).cmp(&pad(&cb)) {
        Ordering::Equal => {}
        o => return Some(o),
    }
    Some(match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            for (p, q) in x.iter().zip(&y) {
                let o = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(m), Ok(n)) => m.cmp(&n),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    _ => p.cmp(q),
                };
                if o != Ordering::Equal {
                    return Some(o);
                }
            }
            x.len().cmp(&y.len())
        }
    })
}

/// npm 上的最新版（`latest` 标签）和查到它的源
async fn fetch_latest(tool: &str) -> Option<Latest> {
    let pkg = crate::tools::package(tool)?;
    let client = reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).timeout(Duration::from_secs(15)).build().ok()?;
    for mirror in ["npmmirror", "official"] {
        let url = format!("{}{}/latest", crate::tools::registry(mirror), pkg.replace('/', "%2f"));
        if let Ok(r) = client.get(&url).send().await.and_then(reqwest::Response::error_for_status) {
            if let Some(v) = r.json::<Value>().await.ok().and_then(|j| j["version"].as_str().map(String::from)) {
                return Some(Latest { version: v, mirror: mirror.to_string() });
            }
        }
    }
    None
}

/// 查一遍三个工具的最新版（查不到的保留上次的）
pub async fn check(store: &Store) -> Result<(), LocalError> {
    let mut found = BTreeMap::new();
    for tool in AGENTS {
        if let Some(l) = fetch_latest(tool).await {
            found.insert(tool.to_string(), l);
        }
    }
    let ok = !found.is_empty();
    edit(store, |s| {
        s.latest.extend(found);
        if ok {
            s.checked_at = Some(now_ms());
        }
    })?;
    if !ok {
        return Err(LocalError::new("AGENT_UPDATE_CHECK_FAILED"));
    }
    Ok(())
}

// ---------------- 在不在用 ----------------

/// 每个工具：正在用它的会话数（在忙的）、打开着的会话数、整台电脑上它的进程数（只在 Windows 上数）
#[derive(Debug, Default, Clone, Copy)]
struct Usage {
    busy: usize,
    open: usize,
    procs: usize,
}

impl Usage {
    /// 现在更新会打断正在用它的
    fn blocks(&self) -> bool {
        self.busy > 0 || self.procs > 0
    }
}

async fn usage(instances: &Instances, chat: &Chat) -> BTreeMap<&'static str, Usage> {
    let mut out: BTreeMap<&'static str, Usage> = AGENTS.iter().map(|t| (*t, Usage::default())).collect();
    for s in crate::idle_stop::list(instances, chat).await {
        let tool = s.tool.or_else(|| AGENTS.iter().copied().find(|t| *t == s.launch));
        if let Some(u) = tool.and_then(|t| out.get_mut(t)) {
            u.open += 1;
            u.busy += s.busy as usize;
        }
    }
    if cfg!(windows) {
        let counts = tokio::task::spawn_blocking(|| {
            let snap = crate::session_probe::Snapshot::take();
            AGENTS.map(|t| crate::session_probe::running_count(&snap, t))
        })
        .await
        .unwrap_or_default();
        for (t, n) in AGENTS.iter().zip(counts) {
            if let Some(u) = out.get_mut(t) {
                u.procs = n;
            }
        }
    }
    out
}

// ---------------- 更新 ----------------

/// 开始更新（`auto`：自动更新起的）。装完在后台收尾：记下结果，Codex 让对话模式换新的，DSH 没在忙时重启网页服务
pub async fn start(store: Arc<Store>, instances: Arc<Instances>, chat: Arc<Chat>, tool: &str, auto: bool) -> Result<crate::tools::Job, LocalError> {
    let tool: &'static str = AGENTS.iter().copied().find(|t| *t == tool).ok_or_else(|| LocalError::invalid("tool", "enum"))?;
    if crate::tools::job(tool).is_some_and(|j| j.state == "running") {
        return Err(LocalError::new("AGENT_UPDATE_RUNNING"));
    }
    let found = installed(instances.shell()).await.remove(tool).flatten().ok_or_else(|| LocalError::new("AGENT_NOT_INSTALLED"))?;
    let saved = load(&store);
    let latest = saved.latest.get(tool).cloned().ok_or_else(|| LocalError::new("AGENT_UPDATE_CHECK_FAILED"))?;
    if found.version.as_deref().is_some_and(|v| !newer(&latest.version, v)) {
        return Err(LocalError::new("AGENT_UP_TO_DATE"));
    }
    let Some((shown, line)) = update_command(tool, &found, &latest.version, &latest.mirror) else {
        return Err(LocalError::new(if found.method == "desktop" { "AGENT_UPDATE_DESKTOP" } else { "AGENT_UPDATE_MANUAL" }));
    };
    tracing::info!(tool, from = ?found.version, to = %latest.version, method = found.method, auto, "更新 AI 工具");
    let job = crate::tools::start_command(instances.shell(), tool, &shown, &line);
    let from = found.version.clone();
    let target = latest.version.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if crate::tools::job(tool).is_none_or(|j| j.state != "running") {
                break;
            }
        }
        let job = crate::tools::job(tool);
        let now = installed(instances.shell()).await.remove(tool).flatten().and_then(|i| i.version);
        let changed = now.is_some() && now != from;
        let (ok, error) = match job.as_ref().map(|j| j.state) {
            Some("ok") if changed || now.is_none() => (true, None),
            Some("ok") => (false, Some("unchanged".to_string())),
            _ => (false, Some(classify(job.as_ref().map(|j| j.output.as_str()).unwrap_or_default()).to_string())),
        };
        tracing::info!(tool, ok, error = ?error, version = ?now, "AI 工具更新结束");
        let outcome = Outcome { target, from, to: now.clone(), ok, error, auto, at: now_ms() };
        if let Err(e) = edit(&store, |s| s.last.insert(tool.to_string(), outcome)) {
            tracing::warn!(error = ?e, "记不下 AI 工具的更新结果");
        }
        if ok {
            after_update(tool, now.as_deref(), &instances, &chat).await;
        }
    });
    Ok(job)
}

/// 装好以后：Claude Code 对话模式的进程、Codex 对话模式的 app-server 空闲时换成新的；DSH 的网页服务（一直开着的）没在忙就重启，换上新版本
async fn after_update(tool: &str, version: Option<&str>, instances: &Instances, chat: &Chat) {
    match tool {
        "claude" => {
            if let Some(v) = version {
                chat.claude_installed(v);
            }
        }
        "codex" => chat.codex_installed(),
        "dsh" => {
            let Ok(Some(row)) = instances.dsh() else { return };
            let Ok(v) = instances.get(&row.id).await else { return };
            if (v.running || v.task_alive == Some(true)) && !crate::idle_stop::busy(chat, &v) {
                match instances.restart(&row.id).await {
                    Ok(_) => tracing::info!("DSH 更新好了，重启了网页服务"),
                    Err(e) => tracing::warn!(error = ?e, "DSH 更新好了，但重启网页服务失败"),
                }
            }
        }
        _ => {}
    }
}

/// 失败的输出 → 原因（界面上给出能照着做的提示）
fn classify(output: &str) -> &'static str {
    let o = output.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| o.contains(w));
    if has(&["enospc", "no space left"]) {
        "disk"
    } else if has(&["ebusy", "resource busy", "being used by another process", "text file busy"]) || (cfg!(windows) && has(&["eperm"])) {
        "busy"
    } else if has(&["eacces", "permission denied", "access is denied", "eperm", "operation not permitted"]) {
        "perm"
    } else if has(&["etimedout", "enotfound", "econnreset", "econnrefused", "eai_again", "socket hang up", "fetch failed", "network", "could not resolve", "timed out", "getaddrinfo"]) {
        "network"
    } else if has(&["unsupported engine", "ebadengine", "requires node", "node.js version"]) {
        "node"
    } else {
        "failed"
    }
}

// ---------------- 界面 ----------------

pub async fn view(store: &Store, instances: &Instances, chat: &Chat) -> Value {
    let (found, usage) = tokio::join!(installed(instances.shell()), usage(instances, chat));
    let saved = load(store);
    let items: Vec<Value> = AGENTS
        .iter()
        .map(|tool| {
            let i = found.get(tool).cloned().flatten();
            let latest = saved.latest.get(*tool);
            let u = usage.get(tool).copied().unwrap_or_default();
            let update = match (&i, latest) {
                (Some(i), Some(l)) => i.version.as_deref().is_none_or(|v| newer(&l.version, v)),
                _ => false,
            };
            let command = match (&i, latest) {
                (Some(i), Some(l)) => update_command(tool, i, &l.version, &l.mirror).map(|c| c.0),
                _ => None,
            };
            // 这种装法看看能不能更新（和查没查到新版本无关）
            let can = i.as_ref().is_some_and(|i| update_command(tool, i, "0", "npmmirror").is_some());
            let job = crate::tools::job(tool);
            json!({
                "tool": tool,
                "installed": i.is_some(),
                "path": i.as_ref().map(|i| &i.path),
                "version": i.as_ref().and_then(|i| i.version.as_ref()),
                "method": i.as_ref().map(|i| i.method),
                "latest": latest.map(|l| &l.version),
                "update": update,
                "can_update": can,
                "command": command,
                "auto": saved.auto.get(*tool).copied().unwrap_or(false),
                "busy": u.busy,
                "open": u.open,
                "procs": u.procs,
                "waiting": u.blocks(),
                "running": job.as_ref().is_some_and(|j| j.state == "running"),
                "last": saved.last.get(*tool),
            })
        })
        .collect();
    json!({ "items": items, "checked_at": saved.checked_at, "os": std::env::consts::OS })
}

pub fn set_auto(store: &Store, tool: &str, on: bool) -> Result<(), LocalError> {
    if !AGENTS.contains(&tool) {
        return Err(LocalError::invalid("tool", "enum"));
    }
    edit(store, |s| {
        s.auto.insert(tool.to_string(), on);
    })
}

// ---------------- 后台 ----------------

/// 定时查新版本；开了自动更新的、有新版本、没在用时更新
pub async fn run(store: Arc<Store>, instances: Arc<Instances>, chat: Arc<Chat>) {
    tokio::time::sleep(FIRST).await;
    loop {
        let saved = load(&store);
        let due = saved.checked_at.is_none_or(|t| now_ms() - t >= CHECK_EVERY.as_millis() as i64);
        if due {
            if let Err(e) = check(&store).await {
                tracing::debug!(error = ?e, "查 AI 工具的新版本失败");
            }
        }
        if let Err(e) = auto_tick(&store, &instances, &chat).await {
            tracing::warn!(error = ?e, "自动更新 AI 工具失败");
        }
        follow_installed(&instances, &chat).await;
        tokio::time::sleep(TICK).await;
    }
}

/// 每个工具上次看到装着的版本（用量页显示“已装 x”，比对话进程新的就提示 / 空闲时换上）
static SEEN: Mutex<BTreeMap<&'static str, String>> = Mutex::new(BTreeMap::new());

/// 现在装着的版本（后台每 [`TICK`] 看一次；还没看过时 None）
pub fn installed_version(tool: &str) -> Option<String> {
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).get(tool).cloned()
}

/// 不管是谁更新的（看看、用户自己、Claude Code 自己后台更新）：装着的版本变了，开着的对话空闲时换上新版本。
/// Claude Code 对话模式按每个进程自己报的版本比；Codex 的 app-server 是共用的，看到版本变了就让它空闲时换
async fn follow_installed(instances: &Instances, chat: &Chat) {
    let found = installed(instances.shell()).await;
    for (tool, i) in found {
        let Some(v) = i.and_then(|i| i.version) else { continue };
        let before = SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(tool, v.clone());
        match tool {
            "claude" => chat.claude_installed(&v),
            "codex" if before.is_some_and(|b| b != v) => chat.codex_installed(),
            _ => {}
        }
    }
}

async fn auto_tick(store: &Arc<Store>, instances: &Arc<Instances>, chat: &Arc<Chat>) -> Result<(), LocalError> {
    let saved = load(store);
    // 开着自动更新、查到过新版本、这个版本没失败过（或者失败够久了）、装完版本没变过的不再试
    let wanted: Vec<&str> = AGENTS
        .iter()
        .copied()
        .filter(|t| saved.auto.get(*t).copied().unwrap_or(false))
        .filter(|t| {
            let Some(l) = saved.latest.get(*t) else { return false };
            match saved.last.get(*t).filter(|o| o.target == l.version) {
                Some(o) if o.ok => false,
                Some(o) if o.error.as_deref() == Some("unchanged") => false,
                Some(o) => now_ms() - o.at >= RETRY_MS,
                None => true,
            }
        })
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let found = installed(instances.shell()).await;
    let pending: Vec<&str> = wanted
        .into_iter()
        .filter(|t| {
            let (Some(Some(i)), Some(l)) = (found.get(t), saved.latest.get(*t)) else { return false };
            i.version.as_deref().is_some_and(|v| newer(&l.version, v)) && update_command(t, i, &l.version, &l.mirror).is_some()
        })
        .collect();
    if pending.is_empty() {
        return Ok(());
    }
    let usage = usage(instances, chat).await;
    for tool in pending {
        if usage.get(tool).is_some_and(Usage::blocks) {
            tracing::debug!(tool, "有新版本，等没有会话在用时再自动更新");
            continue;
        }
        if crate::tools::job(tool).is_some_and(|j| j.state == "running") {
            continue;
        }
        start(store.clone(), instances.clone(), chat.clone(), tool, true).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("2.1.10", "2.1.9"));
        assert!(!newer("2.1.9", "2.1.9"));
        assert!(newer("0.2.2-rc.1", "0.2.1"));
        assert!(newer("0.2.2", "0.2.2-rc.1"));
        assert!(newer("0.2.2-rc.1", "0.2.2-alpha.3"), "rc 比 alpha 新");
        assert!(newer("0.2.2-rc.10", "0.2.2-rc.9"));
        assert!(!newer("0.2.2-rc.1", "0.2.2"));
        assert!(newer("v1.0.1", "1.0"));
        assert!(!newer("garbage", "1.0"));
        assert!(looks_like_version("2.1.322") && looks_like_version("0.2.2-rc.1") && !looks_like_version("codex-cli") && !looks_like_version("2"));
    }

    #[test]
    fn errors() {
        assert_eq!(classify("npm error code EACCES\nnpm error syscall mkdir"), "perm");
        assert_eq!(classify("npm error code ETIMEDOUT"), "network");
        assert_eq!(classify("ENOSPC: no space left on device"), "disk");
        assert_eq!(classify("npm error code EBUSY"), "busy");
        assert_eq!(classify("something odd"), "failed");
    }

    #[cfg(unix)]
    #[test]
    fn locates_install_methods() {
        let dir = std::env::temp_dir().join(format!("ll-agent-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // npm：bin 里的链接指向包里的程序
        let pkg = dir.join("lib/node_modules/@openai/codex");
        std::fs::create_dir_all(pkg.join("bin")).unwrap();
        std::fs::write(pkg.join("package.json"), r#"{"version":"0.164.0"}"#).unwrap();
        std::fs::write(pkg.join("bin/codex.js"), "").unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::os::unix::fs::symlink(pkg.join("bin/codex.js"), dir.join("bin/codex")).unwrap();
        let i = locate("codex", &dir.join("bin/codex"));
        assert_eq!((i.method, i.version.as_deref()), ("npm", Some("0.164.0")));
        let (shown, line) = update_command("codex", &i, "0.165.0", "official").unwrap();
        assert_eq!(shown, "npm install -g @openai/codex@0.165.0 --registry=https://registry.npmjs.org/");
        assert!(line.starts_with(&shown) && line.contains("--loglevel=http"));
        // Claude Code 安装脚本
        let v = dir.join(".local/share/claude/versions/2.1.322");
        std::fs::create_dir_all(v.parent().unwrap()).unwrap();
        std::fs::write(&v, "").unwrap();
        std::os::unix::fs::symlink(&v, dir.join("bin/claude")).unwrap();
        let i = locate("claude", &dir.join("bin/claude"));
        assert_eq!((i.method, i.version.as_deref()), ("native", Some("2.1.322")));
        let (shown, line) = update_command("claude", &i, "2.1.400", "npmmirror").unwrap();
        assert_eq!(shown, "claude update");
        assert!(line.ends_with("/bin/claude' update"), "{line}");
        // Homebrew cask
        let c = dir.join("opt/homebrew/Caskroom/codex/0.160.0/codex-aarch64-apple-darwin");
        std::fs::create_dir_all(c.parent().unwrap()).unwrap();
        std::fs::write(&c, "").unwrap();
        let i = locate("codex", &c);
        assert_eq!((i.method, i.version.as_deref()), ("brew", Some("0.160.0")));
        assert_eq!(update_command("codex", &i, "0.165.0", "npmmirror").unwrap().0, "brew upgrade --cask codex");
        // 认不出的 DSH：不自己更新
        let d = dir.join("bin/dsh");
        std::fs::write(&d, "").unwrap();
        let i = locate("dsh", &d);
        assert_eq!(i.method, "unknown");
        assert!(update_command("dsh", &i, "0.3.0", "npmmirror").is_none());
        // bun
        let b = dir.join(".bun/install/global/node_modules/@deepseek-ai/dsh");
        std::fs::create_dir_all(b.join("lib")).unwrap();
        std::fs::write(b.join("package.json"), r#"{"version":"0.2.1"}"#).unwrap();
        std::fs::write(b.join("lib/bin.js"), "").unwrap();
        let i = locate("dsh", &b.join("lib/bin.js"));
        assert_eq!((i.method, i.version.as_deref()), ("bun", Some("0.2.1")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
