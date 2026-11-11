//! 会话里现在在跑什么：定时（会话列表每次刷新，最多每 [`crate::instances`] 的 TITLE_TTL 一次）看每个会话的进程树，
//! 认出在跑哪个 AI 工具、它正在写哪段对话（会话名就用对话标题）、是不是在忙。普通终端里手动敲了 `claude` / `codex`
//! 也认得出来，会话上就换成那个工具的图标；终端里有命令在跑（不是停在提示符）也标出来。
//!
//! 每个工具一个 [`Detector`]，认法各不相同，新加工具只要实现这个接口、放进 [`detectors`]：
//! - **Claude Code**：每个进程在 `<配置目录>/sessions/<进程号>.json` 里写着当前的会话号和状态（idle / busy / waiting…），
//!   换对话（/clear、/resume）也会更新。多账号时每个账号有自己的配置目录，都看。
//! - **Codex**：没有这样的文件，但它一直开着当前对话的记录文件（`rollout-…-<会话号>.jsonl`）：Linux 读 `/proc/<pid>/fd`，
//!   macOS 用 `lsof`。记录文件几秒内还在写就是在忙。
//! - **DSH、OpenCode**：只认出工具（图标），名字用它们设置的终端标题。
//! - 都不是：普通终端，tmux 报的前台程序不是 shell 就是有命令在跑。
//!
//! 认不出对话号时调用方再按文件夹猜，只取这个进程启动之后**开始**的那段对话——不按“最近有动静”猜：
//! 那样同一文件夹里别处的对话一有动静，这个会话的名字就跟着变成别人的。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// 一个进程（拍快照时的样子）
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// 程序名和命令行前几个参数的文件名（小写，去掉 `.exe`、`.cmd`、`.js`）：认工具用
    names: Vec<String>,
    /// 启动时间（Unix 秒）
    pub start: u64,
}

/// 全部进程的快照：子进程表
pub struct Snapshot {
    procs: HashMap<u32, ProcInfo>,
    children: HashMap<u32, Vec<u32>>,
}

impl Snapshot {
    pub fn take() -> Self {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always).with_exe(UpdateKind::Always));
        let procs: HashMap<u32, ProcInfo> = sys
            .processes()
            .iter()
            .filter(|(_, p)| p.thread_kind().is_none())
            .map(|(pid, p)| {
                // 程序名、argv0；argv0 是解释器（node 跑的 cli.js）时再加上脚本名。别的参数不看（`cat codex` 不算）
                let mut names = vec![stem(&p.name().to_string_lossy())];
                let cmd: Vec<String> = p.cmd().iter().take(2).map(|a| stem(&a.to_string_lossy())).collect();
                if let Some(a0) = cmd.first() {
                    names.push(a0.clone());
                    if INTERPRETERS.contains(&a0.as_str()) || INTERPRETERS.contains(&names[0].as_str()) {
                        names.extend(cmd.get(1).cloned());
                    }
                }
                if let Some(exe) = p.exe() {
                    names.push(exe.to_string_lossy().to_lowercase().replace('\\', "/"));
                }
                (pid.as_u32(), ProcInfo { pid: pid.as_u32(), ppid: p.parent().map(|x| x.as_u32()), names, start: p.start_time() })
            })
            .collect();
        Self::from_procs(procs)
    }

    fn from_procs(procs: HashMap<u32, ProcInfo>) -> Self {
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for p in procs.values() {
            if let Some(pp) = p.ppid {
                children.entry(pp).or_default().push(p.pid);
            }
        }
        for c in children.values_mut() {
            c.sort_unstable();
        }
        Self { procs, children }
    }

    pub fn get(&self, pid: u32) -> Option<&ProcInfo> {
        self.procs.get(&pid)
    }

    /// `root` 的子孙（不含它自己），离它近的在前
    pub fn descendants(&self, root: u32) -> Vec<&ProcInfo> {
        let mut out = vec![];
        let mut queue: VecDeque<u32> = self.children.get(&root).cloned().unwrap_or_default().into();
        while let Some(pid) = queue.pop_front() {
            if out.len() > 2000 {
                break;
            }
            if let Some(p) = self.procs.get(&pid) {
                out.push(p);
            }
            if let Some(c) = self.children.get(&pid) {
                queue.extend(c);
            }
        }
        out
    }
}

/// 跑脚本的程序：命令行第二个参数才是真正的工具
const INTERPRETERS: [&str; 5] = ["node", "bun", "deno", "python3", "python"];

/// 文件名去掉目录和常见扩展名、转小写：`/usr/lib/node_modules/@openai/codex/bin/codex.js` → `codex`
fn stem(s: &str) -> String {
    let base = s.rsplit(['/', '\\']).next().unwrap_or(s).to_lowercase();
    for ext in [".exe", ".cmd", ".js", ".mjs", ".ps1"] {
        if let Some(b) = base.strip_suffix(ext) {
            return b.to_string();
        }
    }
    base
}

/// 看一个会话时手上有的东西
pub struct Ctx<'a> {
    /// 会话面板里的进程（shell 或者直接启动的程序）的子孙，离面板近的在前；面板进程自己在第一个
    pub tree: &'a [&'a ProcInfo],
    /// Claude Code 的各个配置目录（本机默认 + 每个另外登录的账号）
    pub claude_dirs: &'a [PathBuf],
}

/// 认出来的结果
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    /// 工具（`claude` / `codex` / `dsh` / `opencode`，同 history.rs、会话的 launch）
    pub tool: &'static str,
    /// 工具进程的启动时间（Unix 秒）：按文件夹猜对话时只要这之后开始的
    pub since: u64,
    /// 正在写的对话号
    pub conv: Option<String>,
    /// 在忙（在回复、在跑工具）；不知道是 None
    pub busy: Option<bool>,
    /// 用的哪个账号 / 接口（[`Detector::account`]）：用量统计按它算价格、分账号
    pub account: Option<Account>,
}

/// 工具进程用的账号：配置目录（Claude Code 另外登录的官方账号各有一个）、接口地址（第三方接口）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Account {
    pub dir: Option<std::path::PathBuf>,
    pub base_url: Option<String>,
}

/// 一种工具的认法
pub trait Detector: Send + Sync {
    /// 工具名，见 [`Found::tool`]
    fn tool(&self) -> &'static str;
    /// 进程名 / 命令行里出现这些名字就是它
    fn names(&self) -> &'static [&'static str];
    /// 它正在写的对话号和忙不忙；`procs` 是这个工具的进程（外层的包装脚本在前，真正的程序可能在后面）
    fn inspect(&self, _ctx: &Ctx, _procs: &[&ProcInfo]) -> (Option<String>, Option<bool>) {
        (None, None)
    }
    /// 用的哪个账号 / 接口；认不出是 None
    fn account(&self, _ctx: &Ctx, _procs: &[&ProcInfo]) -> Option<Account> {
        None
    }
    /// 树里有没有它：返回它的进程（第一个匹配的及其子孙里同样匹配的）
    fn find<'a>(&self, tree: &[&'a ProcInfo]) -> Vec<&'a ProcInfo> {
        let hit = |p: &ProcInfo| p.names.iter().any(|n| self.names().iter().any(|w| n == w || n.contains(&format!("/{w}/versions/")) || n.ends_with(&format!("/{w}"))));
        tree.iter().copied().filter(|p| hit(p)).collect()
    }
}

/// 认法的先后：先认 AI 工具；普通终端不在这里（[`probe`] 找不到时就是普通终端）
pub fn detectors() -> Vec<Box<dyn Detector>> {
    vec![Box::new(Claude), Box::new(Codex), Box::new(Dsh), Box::new(OpenCode)]
}

/// 整台电脑上在跑的这个工具的进程数（不管是不是看看开的；Claude 桌面版自己的进程不算）。
/// 更新 AI 工具前用（agent_update.rs）：Windows 上程序在运行时文件被占用，换不掉
pub fn running_count(snap: &Snapshot, tool: &str) -> usize {
    let Some(d) = detectors().into_iter().find(|d| d.tool() == tool) else { return 0 };
    let desktop = |p: &ProcInfo| p.names.iter().any(|n| n.contains("claude.app/contents/macos/") || n.contains("/anthropicclaude/"));
    let all: Vec<&ProcInfo> = snap.procs.values().filter(|p| !desktop(p)).collect();
    let hits = d.find(&all);
    // 包装脚本和它起的真正程序算一个：父进程也是它的不再数
    let pids: std::collections::HashSet<u32> = hits.iter().map(|p| p.pid).collect();
    hits.iter().filter(|p| p.ppid.is_none_or(|pp| !pids.contains(&pp))).count()
}

/// 看一个会话：树里第一个（离面板最近的）认得出的工具
pub fn probe(dets: &[Box<dyn Detector>], ctx: &Ctx) -> Option<Found> {
    let mut best: Option<(usize, &Box<dyn Detector>, Vec<&ProcInfo>)> = None;
    for d in dets {
        let procs = d.find(ctx.tree);
        let Some(first) = procs.first() else { continue };
        let at = ctx.tree.iter().position(|p| p.pid == first.pid).unwrap_or(usize::MAX);
        if best.as_ref().is_none_or(|(b, _, _)| at < *b) {
            best = Some((at, d, procs));
        }
    }
    let (_, d, procs) = best?;
    let (conv, busy) = d.inspect(ctx, &procs);
    let account = d.account(ctx, &procs);
    Some(Found { tool: d.tool(), since: procs[0].start, conv, busy, account })
}

/// 一般 shell 的名字：tmux 报的前台程序是这些就是停在提示符
pub fn is_shell(cmd: &str) -> bool {
    matches!(stem(cmd).trim_start_matches('-'), "bash" | "zsh" | "sh" | "fish" | "dash" | "ksh" | "tcsh" | "csh" | "nu" | "pwsh" | "powershell" | "cmd" | "login" | "elvish" | "xonsh")
}

// ---------------- Claude Code ----------------

struct Claude;

impl Detector for Claude {
    fn tool(&self) -> &'static str {
        "claude"
    }
    fn names(&self) -> &'static [&'static str] {
        &["claude", "claude-code"]
    }
    fn inspect(&self, ctx: &Ctx, procs: &[&ProcInfo]) -> (Option<String>, Option<bool>) {
        // 包装脚本（npm 装的 node cli.js）和真正的进程都试：sessions 文件按写它的那个进程号命名
        for p in procs.iter().rev() {
            for dir in ctx.claude_dirs {
                let Ok(text) = std::fs::read_to_string(dir.join("sessions").join(format!("{}.json", p.pid))) else { continue };
                if let Some(r) = claude_session(&text, p.pid) {
                    return r;
                }
            }
        }
        (None, None)
    }
    fn account(&self, ctx: &Ctx, procs: &[&ProcInfo]) -> Option<Account> {
        // 写 sessions/<pid>.json 的那个配置目录就是它的账号；第三方接口看进程的 ANTHROPIC_BASE_URL
        let dir = procs.iter().rev().find_map(|p| ctx.claude_dirs.iter().find(|d| d.join("sessions").join(format!("{}.json", p.pid)).is_file()).cloned());
        let base_url = procs.iter().rev().find_map(|p| environ_var(p.pid, "ANTHROPIC_BASE_URL")).filter(|u| !u.is_empty());
        (dir.is_some() || base_url.is_some()).then_some(Account { dir, base_url })
    }
}

/// 一个进程的环境变量（只能读自己用户的进程）
fn environ_var(pid: u32, key: &str) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let want = format!("{key}=");
        raw.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(want.as_bytes()).map(|v| String::from_utf8_lossy(v).into_owned()))
    }
    #[cfg(not(target_os = "linux"))]
    {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
        let mut sys = System::new();
        let pid = Pid::from_u32(pid);
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), false, ProcessRefreshKind::nothing().with_environ(UpdateKind::Always));
        let want = format!("{key}=");
        sys.process(pid)?.environ().iter().find_map(|kv| kv.to_string_lossy().strip_prefix(&want).map(String::from))
    }
}

/// `sessions/<pid>.json` → (会话号, 忙不忙)。进程号对不上（别的机器、复用了进程号）不算
fn claude_session(text: &str, pid: u32) -> Option<(Option<String>, Option<bool>)> {
    let v: Value = serde_json::from_str(text).ok()?;
    if v["pid"].as_u64() != Some(pid as u64) {
        return None;
    }
    let conv = v["sessionId"].as_str().filter(|s| crate::history::valid_id(s)).map(String::from);
    // idle：等你说话；waiting：等你确认（要你处理，不算在忙）；其他（busy、shell 在跑命令）在忙
    let busy = v["status"].as_str().map(|s| !matches!(s, "idle" | "waiting"));
    Some((conv, busy))
}

// ---------------- Codex ----------------

struct Codex;

/// 记录文件这么久之内写过就当在忙
const CODEX_BUSY: Duration = Duration::from_secs(4);

impl Detector for Codex {
    fn tool(&self) -> &'static str {
        "codex"
    }
    fn names(&self) -> &'static [&'static str] {
        &["codex"]
    }
    fn inspect(&self, _ctx: &Ctx, procs: &[&ProcInfo]) -> (Option<String>, Option<bool>) {
        let pids: Vec<u32> = procs.iter().map(|p| p.pid).collect();
        let Some(path) = open_rollout(&pids) else { return (None, None) };
        let conv = rollout_thread(&path);
        let busy = std::fs::metadata(&path).ok().and_then(|m| m.modified().ok()).map(|t| SystemTime::now().duration_since(t).unwrap_or_default() < CODEX_BUSY);
        (conv, busy)
    }
}

/// `rollout-2026-10-30T10-00-00-<会话号>.jsonl` → 会话号（36 位的 UUID）
fn rollout_thread(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.strip_suffix(".jsonl")?;
    if !name.starts_with("rollout-") || name.len() < 36 + 9 {
        return None;
    }
    let id = &name[name.len() - 36..];
    (id.chars().filter(|c| *c == '-').count() == 4 && crate::history::valid_id(id)).then(|| id.to_string())
}

/// 这些进程开着的 Codex 记录文件（最近写过的那个）
fn open_rollout(pids: &[u32]) -> Option<PathBuf> {
    let files = open_files(pids);
    files
        .into_iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl")))
        .max_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH))
}

#[cfg(target_os = "linux")]
fn open_files(pids: &[u32]) -> Vec<PathBuf> {
    pids.iter()
        .flat_map(|pid| std::fs::read_dir(format!("/proc/{pid}/fd")).into_iter().flatten().flatten())
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .collect()
}

#[cfg(target_os = "macos")]
fn open_files(pids: &[u32]) -> Vec<PathBuf> {
    if pids.is_empty() {
        return vec![];
    }
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    // -Fn：每个文件一行 `n<路径>`；只看普通文件
    let Ok(out) = std::process::Command::new("lsof").args(["-n", "-P", "-Fn", "-a", "-d", "^txt,^mem", "-p", &list]).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.strip_prefix('n')).map(PathBuf::from).collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn open_files(_pids: &[u32]) -> Vec<PathBuf> {
    vec![]
}

// ---------------- 只认工具的 ----------------

struct Dsh;

impl Detector for Dsh {
    fn tool(&self) -> &'static str {
        "dsh"
    }
    fn names(&self) -> &'static [&'static str] {
        &["dsh"]
    }
}

struct OpenCode;

impl Detector for OpenCode {
    fn tool(&self) -> &'static str {
        "opencode"
    }
    fn names(&self) -> &'static [&'static str] {
        &["opencode"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: u32, names: &[&str]) -> ProcInfo {
        ProcInfo { pid, ppid: Some(ppid), names: names.iter().map(|n| stem(n)).collect(), start: pid as u64 }
    }

    #[test]
    fn finds_nearest_tool() {
        // bash → node codex.js → codex（真正的程序）；另一个分支里 claude 起的 bash 不算
        let snap = Snapshot::from_procs(
            [proc(1, 0, &["bash"]), proc(2, 1, &["node", "node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"]), proc(3, 2, &["codex", "/opt/codex/codex"]), proc(4, 3, &["bash"])]
                .into_iter()
                .map(|p| (p.pid, p))
                .collect(),
        );
        let tree: Vec<&ProcInfo> = std::iter::once(&snap.procs[&1]).chain(snap.descendants(1)).collect();
        let ctx = Ctx { tree: &tree, claude_dirs: &[] };
        let f = probe(&detectors(), &ctx).unwrap();
        assert_eq!((f.tool, f.since), ("codex", 2));
        let none: Vec<&ProcInfo> = vec![&snap.procs[&1]];
        assert!(probe(&detectors(), &Ctx { tree: &none, claude_dirs: &[] }).is_none());
    }

    #[test]
    fn claude_native_install_path() {
        // 原生安装：argv0 是 claude，程序本身叫版本号
        let p = ProcInfo { pid: 9, ppid: Some(1), names: vec!["2.1.317".into(), "claude".into(), "/root/.local/share/claude/versions/2.1.317".into()], start: 0 };
        assert_eq!(Claude.find(&[&p]).len(), 1);
        let q = ProcInfo { pid: 9, ppid: Some(1), names: vec!["2.1.317".into(), "/root/.local/share/claude/versions/2.1.317".into()], start: 0 };
        assert_eq!(Claude.find(&[&q]).len(), 1);
        assert!(Codex.find(&[&p]).is_empty());
    }

    #[test]
    fn claude_session_file() {
        let text = r#"{"pid":105111,"sessionId":"037be91f-8dd2-4144-b461-c4a93fc358aa","status":"busy"}"#;
        assert_eq!(claude_session(text, 105111), Some((Some("037be91f-8dd2-4144-b461-c4a93fc358aa".into()), Some(true))));
        assert_eq!(claude_session(text, 1), None);
        assert_eq!(claude_session(r#"{"pid":1,"sessionId":"x","status":"waiting"}"#, 1), Some((Some("x".into()), Some(false))));
    }

    #[test]
    fn rollout_name() {
        let p = Path::new("/h/.codex/sessions/2026/10/30/rollout-2026-10-30T10-00-00-01a0bab1-b566-7fe2-8fc7-7d0501db4186.jsonl");
        assert_eq!(rollout_thread(p).as_deref(), Some("01a0bab1-b566-7fe2-8fc7-7d0501db4186"));
        assert_eq!(rollout_thread(Path::new("/x/notes.jsonl")), None);
    }

    #[test]
    fn shells() {
        assert!(is_shell("-zsh") && is_shell("bash") && is_shell("pwsh.exe"));
        assert!(!is_shell("npm") && !is_shell("vim"));
    }

    /// 这台电脑上看看的 tmux 会话都认一遍，打印出来（只读）：`cargo test probe_live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_live() {
        let mux = std::env::var("LL_MUX").unwrap_or_else(|_| "tmux".into());
        let out = std::process::Command::new(mux).args(["-L", "looklook", "list-panes", "-a", "-F", "#{session_name} #{pane_pid} #{pane_current_command}"]).output().unwrap();
        let snap = Snapshot::take();
        let mut dirs = vec![dirs::home_dir().unwrap().join(".claude")];
        dirs.extend(std::fs::read_dir(dirs::home_dir().unwrap().join(".local/share/looklook/claude-accounts")).into_iter().flatten().flatten().map(|e| e.path()));
        for l in String::from_utf8_lossy(&out.stdout).lines() {
            let mut it = l.split(' ');
            let (name, pid, cmd) = (it.next().unwrap(), it.next().unwrap().parse::<u32>().unwrap(), it.next().unwrap_or(""));
            let tree: Vec<&ProcInfo> = snap.get(pid).into_iter().chain(snap.descendants(pid)).collect();
            println!("{name} {cmd:10} {:?}", probe(&detectors(), &Ctx { tree: &tree, claude_dirs: &dirs }));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sees_own_open_files() {
        let dir = std::env::temp_dir().join(format!("llprobe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("rollout-2026-10-30T10-00-00-01a0bab1-b566-7fe2-8fc7-7d0501db4186.jsonl");
        let _f = std::fs::File::create(&path).unwrap();
        assert_eq!(open_rollout(&[std::process::id()]).as_deref(), Some(path.as_path()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
