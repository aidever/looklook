//! 本机资源数据（管理台“系统”页）：CPU、内存、交换区、负载、网络、磁盘与占用最多的进程。
//!
//! - 每 5 秒采样一次，保留最近 1 小时的逐点数据和最近 24 小时的逐分钟平均值（只在内存里，重启后从头开始）。
//! - 进程列表开销较大（要遍历所有进程），只在最近 30 秒内有人打开“系统”页时才刷新。
//! - 磁盘每分钟刷新一次。
//! - 看看自己占了多少（[`Looklook`]）：和进程列表一起、只在有人看时算。按进程树分给各个模块——
//!   每个终端（ttyd、tmux 面板里的程序和它们的子孙、对话模式另起的 Claude Code）、tmux 服务进程、
//!   客户端主进程、其余由看看起的辅助进程（Codex app-server 等）。内存是各进程常驻内存（RSS）相加，共享的部分会重复算。
//!   Codex 对话起的后台命令虽然挂在 app-server 下面，也算进各自的终端，并单独列出（能停止）；
//!   辅助进程按程序名合并成几组（Codex 每加载一段对话就起一套 MCP 等进程，单个很小、加起来很多）。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sysinfo::{Disks, Networks, Process, ProcessRefreshKind, ProcessesToUpdate, System};

const INTERVAL: Duration = Duration::from_secs(5);
/// 逐点数据保留 1 小时
const RECENT_LEN: usize = 720;
/// 逐分钟数据保留 24 小时
const MINUTES_LEN: usize = 1440;
/// 有人在看“系统”页的判断时长
const WATCH_MS: i64 = 30_000;
const TOP_PROCESSES: usize = 8;
/// 辅助进程按程序名合并后最多列几组
const HELPER_GROUPS: usize = 8;
const MAX_DISKS: usize = 8;

/// 一个采样点（图表用）。百分比为 0–100；网络为每秒字节数。
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Sample {
    /// 毫秒时间戳
    pub t: i64,
    pub cpu: f32,
    pub mem: f32,
    pub rx: u64,
    pub tx: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    pub mount: String,
    pub fs: String,
    pub total: u64,
    pub available: u64,
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    /// 占整机 CPU 的百分比（0–100，已按核数折算）
    pub cpu: f32,
    pub mem: u64,
}

/// 看看的进程从哪几个开始算（[`crate::instances::Instances::process_roots`]、[`crate::chat::Chat::process_roots`]）
#[derive(Debug, Clone, Default)]
pub struct Roots {
    /// tmux 服务进程
    pub mux: Option<u32>,
    pub sessions: Vec<SessionRoots>,
    /// 不是客户端子进程、但是看看起的（Codex app-server 单独运行）：名称、进程号
    pub helpers: Vec<(String, u32)>,
    /// Codex app-server 里加载着几段对话
    pub codex_loaded: Option<usize>,
    /// Codex 对话起的后台命令
    pub codex_bg: Vec<BgRoot>,
}

#[derive(Debug, Clone, Default)]
pub struct BgRoot {
    /// 终端 id
    pub session: String,
    /// app-server 里的进程号（停止时用）
    pub process_id: String,
    pub command: String,
    /// 系统进程号：它和它的子孙都算这条命令的
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionRoots {
    pub id: String,
    pub name: String,
    pub launch: String,
    /// ttyd、tmux 面板、对话模式的 Claude Code……：它们和它们的子孙都算这个终端的
    pub roots: Vec<u32>,
}

/// 一组进程加起来的占用
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct Usage {
    /// 占整机 CPU 的百分比（已按核数折算）
    pub cpu: f32,
    pub mem: u64,
    pub procs: u32,
}

impl Usage {
    fn add(&mut self, cpu: f32, mem: u64) {
        self.cpu += cpu;
        self.mem += mem;
        self.procs += 1;
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionUse {
    pub id: String,
    pub name: String,
    pub launch: String,
    /// 有进程在跑（停止的终端为 false，占用为 0）
    pub running: bool,
    #[serde(flatten)]
    pub usage: Usage,
    /// 这个终端里占内存最多的程序
    pub top: Option<String>,
}

/// 辅助进程里同名的一组
#[derive(Debug, Clone, Default, Serialize)]
pub struct Group {
    pub name: String,
    #[serde(flatten)]
    pub usage: Usage,
}

/// Codex 对话的一条后台命令
#[derive(Debug, Clone, Default, Serialize)]
pub struct BgUse {
    pub session: String,
    pub process_id: String,
    pub command: String,
    pub pid: Option<u32>,
    #[serde(flatten)]
    pub usage: Usage,
}

/// 看看自己（客户端和它管着的进程）的占用，按模块分
#[derive(Debug, Clone, Default, Serialize)]
pub struct Looklook {
    pub total: Usage,
    /// 客户端主进程
    pub main: Usage,
    pub main_pid: u32,
    /// 主进程的线程数（只有 Linux 有）
    pub main_threads: Option<usize>,
    /// 主进程运行了多少秒
    pub main_uptime: u64,
    /// tmux 服务进程（直接运行、没有 tmux 时为 None）
    pub mux: Option<Usage>,
    /// 其余辅助进程（Codex app-server、主进程起的其他程序）
    pub helpers: Usage,
    /// 辅助进程按程序名合并，按内存从多到少，最多 [`HELPER_GROUPS`] 组
    pub helper_groups: Vec<Group>,
    /// Codex app-server 里加载着几段对话（没起 app-server 时为 None）
    pub codex_loaded: Option<usize>,
    /// Codex 对话的后台命令（也算在各自的终端里）
    pub background: Vec<BgUse>,
    /// 所有终端（停止的也在，按内存从多到少）
    pub sessions: Vec<SessionUse>,
}

/// 看看的总占用（看板上的曲线，只在有人看时记）
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct OwnSample {
    pub t: i64,
    pub cpu: f32,
    pub mem: u64,
}

#[derive(Clone, Copy, PartialEq)]
enum Owner {
    Main,
    Mux,
    Helper,
    Session(usize),
    /// Codex 后台命令（第几条，所在的终端）
    Bg(usize, usize),
}

/// 按进程树把进程分给各个模块。`me`：客户端自己的进程号（客户端可能是在某个终端里启动的，那样它和它的子孙也不算那个终端的）。
fn breakdown(procs: &HashMap<u32, &Process>, roots: &Roots, me: u32, cores: f32) -> Looklook {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, p) in procs {
        if let Some(pp) = p.parent() {
            children.entry(pp.as_u32()).or_default().push(pid);
        }
    }
    let mut owner: HashMap<u32, Owner> = HashMap::new();
    owner.insert(me, Owner::Main);
    if let Some(m) = roots.mux {
        owner.insert(m, Owner::Mux);
    }
    // 已经分过的不再往下走（先到先得：终端优先，剩下的才算辅助进程）
    let walk = |owner: &mut HashMap<u32, Owner>, start: u32, o: Owner| {
        let mut stack = vec![start];
        while let Some(p) = stack.pop() {
            if owner.contains_key(&p) || !procs.contains_key(&p) {
                continue;
            }
            owner.insert(p, o);
            stack.extend(children.get(&p).into_iter().flatten());
        }
    };
    for (i, s) in roots.sessions.iter().enumerate() {
        for &r in &s.roots {
            walk(&mut owner, r, Owner::Session(i));
        }
    }
    // Codex 后台命令挂在 app-server 下面：先于辅助进程分给各自的终端
    let mut bgs: Vec<BgUse> = vec![];
    for b in &roots.codex_bg {
        let (Some(pid), Some(i)) = (b.pid, roots.sessions.iter().position(|s| s.id == b.session)) else { continue };
        if owner.contains_key(&pid) {
            continue;
        }
        walk(&mut owner, pid, Owner::Bg(bgs.len(), i));
        bgs.push(BgUse { session: b.session.clone(), process_id: b.process_id.clone(), command: b.command.clone(), pid: Some(pid), usage: Usage::default() });
    }
    for &(_, pid) in &roots.helpers {
        walk(&mut owner, pid, Owner::Helper);
    }
    for &c in children.get(&me).into_iter().flatten() {
        walk(&mut owner, c, Owner::Helper);
    }
    // tmux 服务进程下面不属于任何终端的（刚关掉的面板之类）
    for &c in roots.mux.and_then(|m| children.get(&m)).into_iter().flatten() {
        walk(&mut owner, c, Owner::Mux);
    }
    let mut out = Looklook { main_pid: me, codex_loaded: roots.codex_loaded, ..Default::default() };
    let mut sessions: Vec<(Usage, Option<(u64, String)>)> = vec![Default::default(); roots.sessions.len()];
    let mut mux = roots.mux.filter(|m| procs.contains_key(m)).map(|_| Usage::default());
    let mut groups: Vec<Group> = vec![];
    for (&pid, &o) in &owner {
        let Some(p) = procs.get(&pid) else { continue };
        let (cpu, mem) = (p.cpu_usage() / cores, p.memory());
        out.total.add(cpu, mem);
        match o {
            Owner::Main => {
                out.main.add(cpu, mem);
                out.main_threads = p.tasks().map(|t| t.len());
                out.main_uptime = p.run_time();
            }
            Owner::Mux => mux.get_or_insert_with(Usage::default).add(cpu, mem),
            Owner::Helper => {
                out.helpers.add(cpu, mem);
                // npm 装的 codex：app-server 的进程号是 node 外壳，下面那个真的 codex 也算同一组
                let own = p.name().to_string_lossy().into_owned();
                let parent = p.parent().map(|pp| pp.as_u32());
                let name = roots
                    .helpers
                    .iter()
                    .find(|h| h.1 == pid || (Some(h.1) == parent && own.to_ascii_lowercase().starts_with("codex")))
                    .map(|h| h.0.clone())
                    .unwrap_or(own);
                match groups.iter_mut().find(|g| g.name == name) {
                    Some(g) => g.usage.add(cpu, mem),
                    None => {
                        let mut g = Group { name, usage: Usage::default() };
                        g.usage.add(cpu, mem);
                        groups.push(g);
                    }
                }
            }
            Owner::Session(i) | Owner::Bg(_, i) => {
                if let Owner::Bg(k, _) = o {
                    bgs[k].usage.add(cpu, mem);
                }
                let (u, top) = &mut sessions[i];
                u.add(cpu, mem);
                if top.as_ref().is_none_or(|(m, _)| mem > *m) {
                    *top = Some((mem, p.name().to_string_lossy().into_owned()));
                }
            }
        }
    }
    out.mux = mux;
    groups.sort_by_key(|g| std::cmp::Reverse(g.usage.mem));
    groups.truncate(HELPER_GROUPS);
    out.helper_groups = groups;
    // 已经结束的（进程表里没有）不列
    bgs.retain(|b| b.usage.procs > 0);
    bgs.sort_by_key(|b| std::cmp::Reverse(b.usage.mem));
    out.background = bgs;
    out.sessions = roots
        .sessions
        .iter()
        .zip(sessions)
        .map(|(s, (usage, top))| SessionUse { id: s.id.clone(), name: s.name.clone(), launch: s.launch.clone(), running: usage.procs > 0, usage, top: top.map(|t| t.1) })
        .collect();
    out.sessions.sort_by(|a, b| b.running.cmp(&a.running).then(b.usage.mem.cmp(&a.usage.mem)));
    out
}

/// 当前状态（统计卡片与列表用）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct Now {
    pub t: i64,
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub cpu_brand: String,
    pub mem_total: u64,
    pub mem_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// 1、5、15 分钟平均负载（Windows 没有）
    pub load: Option<[f64; 3]>,
    pub uptime: u64,
    pub rx: u64,
    pub tx: u64,
    pub disks: Vec<Disk>,
    /// 没人看时不刷新进程，为 None
    pub processes: Option<usize>,
    pub top: Vec<Proc>,
    /// 看看自己的占用（没人看时不算，为 None）
    pub looklook: Option<Looklook>,
}

struct Sampler {
    sys: System,
    nets: Networks,
    disks: Disks,
    disks_at: i64,
    net_at: i64,
    procs_fresh: bool,
}

#[derive(Default)]
struct History {
    now: Now,
    recent: VecDeque<Sample>,
    minutes: VecDeque<Sample>,
    /// 当前这一分钟里的采样（凑满一分钟求平均后进 `minutes`）
    minute: Vec<Sample>,
    /// 看看的总占用（有人看时才有，最近 1 小时）
    own: VecDeque<OwnSample>,
}

pub struct Metrics {
    history: Mutex<History>,
    watched_at: AtomicI64,
}

fn now_ms() -> i64 {
    crate::util::system_ms()
}

/// 虚拟网卡（本机回环、容器、虚拟机网桥）不算进网速，否则容器之间的流量会被重复计算。
fn counted_interface(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    !(n == "lo" || n.starts_with("lo") && n[2..].chars().all(|c| c.is_ascii_digit()) || n.starts_with("loopback") || ["veth", "docker", "br-", "virbr", "vethernet", "cni", "flannel", "utun", "awdl", "llw"].iter().any(|p| n.starts_with(p)))
}

/// 系统内部的挂载（内存盘、只读的软件包镜像等）不显示。
fn counted_disk(fs: &str, mount: &str) -> bool {
    let fs = fs.to_ascii_lowercase();
    let skip_fs = ["tmpfs", "devtmpfs", "squashfs", "overlay", "proc", "sysfs", "efivarfs", "ramfs", "autofs", "devfs", "nullfs"];
    !skip_fs.contains(&fs.as_str()) && !mount.starts_with("/snap/") && !mount.starts_with("/boot/efi") && !mount.starts_with("/System/Volumes/") && !mount.starts_with("/private/var/vm")
}

fn mean(xs: &[Sample]) -> Sample {
    let n = xs.len().max(1);
    Sample {
        t: xs.last().map(|s| s.t).unwrap_or_default(),
        cpu: xs.iter().map(|s| s.cpu).sum::<f32>() / n as f32,
        mem: xs.iter().map(|s| s.mem).sum::<f32>() / n as f32,
        rx: xs.iter().map(|s| s.rx).sum::<u64>() / n as u64,
        tx: xs.iter().map(|s| s.tx).sum::<u64>() / n as u64,
    }
}

impl Sampler {
    fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        Self { sys, nets: Networks::new_with_refreshed_list(), disks: Disks::new(), disks_at: 0, net_at: now_ms(), procs_fresh: false }
    }

    fn sample(&mut self, watched: bool, roots: Option<&Roots>) -> Now {
        let t = now_ms();
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        // 网卡可能增减（插网线、开 VPN），一起刷新列表
        self.nets.refresh(true);
        let secs = ((t - self.net_at).max(1) as f64) / 1000.0;
        self.net_at = t;
        let (rx, tx) = self.nets.iter().filter(|(name, _)| counted_interface(name)).fold((0u64, 0u64), |(r, w), (_, d)| (r + d.received(), w + d.transmitted()));
        if t - self.disks_at >= 60_000 {
            self.disks.refresh(true);
            self.disks_at = t;
        }
        let mut seen = std::collections::HashSet::new();
        let disks: Vec<Disk> = self
            .disks
            .iter()
            .filter(|d| d.total_space() > 0 && counted_disk(&d.file_system().to_string_lossy(), &d.mount_point().to_string_lossy()))
            // 同一块盘挂在多处（bind mount）只算一次
            .filter(|d| seen.insert((d.name().to_os_string(), d.total_space())))
            .take(MAX_DISKS)
            .map(|d| Disk {
                mount: d.mount_point().display().to_string(),
                fs: d.file_system().to_string_lossy().into_owned(),
                total: d.total_space(),
                available: d.available_space(),
                removable: d.is_removable(),
            })
            .collect();
        let cores = self.sys.cpus().iter().map(|c| c.cpu_usage()).collect::<Vec<_>>();
        let (mut processes, mut top, mut looklook) = (None, vec![], None);
        if watched {
            self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
            // 进程 CPU 是两次刷新之间算出来的，刚开始看时第一次的数字不准，先不显示
            if self.procs_fresh {
                let n = cores.len().max(1) as f32;
                let mut all: Vec<Proc> = self
                    .sys
                    .processes()
                    .values()
                    // Linux 上线程也会列出来，只要进程
                    .filter(|p| p.thread_kind().is_none())
                    .map(|p| Proc { pid: p.pid().as_u32(), name: p.name().to_string_lossy().into_owned(), cpu: p.cpu_usage() / n, mem: p.memory() })
                    .collect();
                processes = Some(all.len());
                if let Some(roots) = roots {
                    let procs: HashMap<u32, &Process> = self.sys.processes().iter().filter(|(_, p)| p.thread_kind().is_none()).map(|(pid, p)| (pid.as_u32(), p)).collect();
                    looklook = Some(breakdown(&procs, roots, std::process::id(), n));
                }
                all.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.mem.cmp(&a.mem)));
                all.truncate(TOP_PROCESSES);
                top = all;
            }
            self.procs_fresh = true;
        } else if self.procs_fresh {
            // 没人看了：丢掉进程表，省内存
            self.sys = System::new();
            self.sys.refresh_cpu_all();
            self.procs_fresh = false;
        }
        let load = System::load_average();
        let mem_total = self.sys.total_memory();
        Now {
            t,
            cpu: self.sys.global_cpu_usage(),
            cores,
            cpu_brand: self.sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default(),
            mem_total,
            // 已用 = 总量 − 可用（缓存可以随时释放，不算占用，与系统自带的任务管理器一致）
            mem_used: mem_total.saturating_sub(self.sys.available_memory()),
            swap_total: self.sys.total_swap(),
            swap_used: self.sys.used_swap(),
            load: (!cfg!(windows)).then_some([load.one, load.five, load.fifteen]),
            uptime: System::uptime(),
            rx: (rx as f64 / secs) as u64,
            tx: (tx as f64 / secs) as u64,
            disks,
            processes,
            top,
            looklook,
        }
    }
}

impl Metrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self { history: Mutex::new(History::default()), watched_at: AtomicI64::new(0) })
    }

    fn history(&self) -> std::sync::MutexGuard<'_, History> {
        self.history.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn watched(&self) -> bool {
        now_ms() - self.watched_at.load(Ordering::Relaxed) < WATCH_MS
    }

    /// 后台采样（随 run() 收尾时 abort）。`instances`、`chat`：有人看时问它们看看的进程从哪开始（资源看板）。
    pub async fn run(self: Arc<Self>, instances: Arc<crate::instances::Instances>, chat: Arc<crate::chat::Chat>) {
        let sampler = Arc::new(Mutex::new(Sampler::new()));
        let mut tick = tokio::time::interval(INTERVAL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let (s, watched) = (sampler.clone(), self.watched());
            let roots = if watched { Some(own_roots(&instances, &chat).await) } else { None };
            // 遍历进程在 Windows 上可能要上百毫秒，不占用异步线程
            let Ok(now) = tokio::task::spawn_blocking(move || s.lock().unwrap_or_else(|e| e.into_inner()).sample(watched, roots.as_ref())).await else { continue };
            self.record(now);
        }
    }

    fn record(&self, now: Now) {
        let s = Sample { t: now.t, cpu: now.cpu, mem: pct(now.mem_used, now.mem_total), rx: now.rx, tx: now.tx };
        let mut h = self.history();
        if let Some(l) = &now.looklook {
            let o = OwnSample { t: now.t, cpu: l.total.cpu, mem: l.total.mem };
            if h.own.len() == RECENT_LEN {
                h.own.pop_front();
            }
            h.own.push_back(o);
        }
        h.now = now;
        push(&mut h.recent, s, RECENT_LEN);
        // 换了一分钟：把上一分钟的平均值存起来
        if h.minute.first().is_some_and(|f| f.t / 60_000 != s.t / 60_000) {
            let avg = mean(&h.minute);
            push(&mut h.minutes, avg, MINUTES_LEN);
            h.minute.clear();
        }
        h.minute.push(s);
    }

    /// 概要（`/api/status`，侧栏显示）：CPU 与内存百分比。还没采到数据时为 None。
    pub fn brief(&self) -> Option<serde_json::Value> {
        let h = self.history();
        (h.now.t > 0).then(|| serde_json::json!({ "cpu": h.now.cpu, "mem": pct(h.now.mem_used, h.now.mem_total) }))
    }

    /// “系统”页：当前状态与一段时间的曲线。`range`：`5m`、`1h`、`24h`。
    pub fn view(&self, range: &str) -> serde_json::Value {
        self.watched_at.store(now_ms(), Ordering::Relaxed);
        let h = self.history();
        let (series, step): (Vec<Sample>, u64) = match range {
            "24h" => {
                let mut v: Vec<Sample> = h.minutes.iter().copied().collect();
                if !h.minute.is_empty() {
                    v.push(mean(&h.minute));
                }
                (v, 60)
            }
            "1h" => (h.recent.iter().copied().collect(), INTERVAL.as_secs()),
            _ => (h.recent.iter().skip(h.recent.len().saturating_sub(60)).copied().collect(), INTERVAL.as_secs()),
        };
        // 看看自己的曲线：只有有人看时才记，按时间截取
        let span = match range {
            "24h" => 86_400_000,
            "1h" => 3_600_000,
            _ => 300_000,
        };
        let from = h.now.t - span;
        let own: Vec<OwnSample> = h.own.iter().filter(|o| o.t >= from).copied().collect();
        serde_json::json!({ "now": h.now, "series": series, "step": step, "own": own })
    }
}

/// 看看的进程从哪几个开始算：终端（加上对话模式另起的 Claude Code）、tmux 服务进程、Codex app-server
async fn own_roots(instances: &crate::instances::Instances, chat: &crate::chat::Chat) -> Roots {
    let mut roots = instances.process_roots().await;
    let chat_roots = chat.process_roots().await;
    for (id, pid) in chat_roots.sdk {
        if let Some(s) = roots.sessions.iter_mut().find(|s| s.id == id) {
            s.roots.push(pid);
        }
    }
    roots.helpers.extend(chat_roots.codex.map(|p| ("Codex app-server".to_string(), p)));
    roots.codex_loaded = chat_roots.codex_loaded;
    roots.codex_bg = chat_roots.codex_bg.into_iter().map(|b| BgRoot { session: b.session, process_id: b.process_id, command: b.command, pid: b.pid }).collect();
    roots
}

fn pct(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 * 100.0 / total as f64) as f32
    }
}

fn push(q: &mut VecDeque<Sample>, s: Sample, cap: usize) {
    if q.len() == cap {
        q.pop_front();
    }
    q.push_back(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_and_disks() {
        assert!(counted_interface("eth0") && counted_interface("en0") && counted_interface("Wi-Fi") && counted_interface("wlan0"));
        assert!(!counted_interface("lo") && !counted_interface("lo0") && !counted_interface("docker0") && !counted_interface("veth12ab") && !counted_interface("br-1a2b"));
        assert!(counted_interface("long-name"), "以 lo 开头但不是回环");
        assert!(counted_disk("ext4", "/") && counted_disk("NTFS", "C:\\") && counted_disk("apfs", "/"));
        assert!(!counted_disk("tmpfs", "/run") && !counted_disk("squashfs", "/snap/core/1") && !counted_disk("overlay", "/"));
    }

    #[test]
    fn minutes_are_averaged() {
        let m = Metrics::new();
        let base = 1_700_000_040_000; // 整分钟
        for (i, cpu) in [10.0, 30.0, 50.0].into_iter().enumerate() {
            m.record(Now { t: base + i as i64 * 5000, cpu, mem_total: 100, mem_used: 50, ..Default::default() });
        }
        m.record(Now { t: base + 60_000, cpu: 90.0, mem_total: 100, mem_used: 50, ..Default::default() });
        let v = m.view("24h");
        let s = v["series"].as_array().unwrap();
        assert_eq!(s.len(), 2, "一个整分钟 + 当前未满的一分钟");
        assert_eq!(s[0]["cpu"].as_f64().unwrap(), 30.0);
        assert_eq!(s[0]["mem"].as_f64().unwrap(), 50.0);
        assert_eq!(s[1]["cpu"].as_f64().unwrap(), 90.0);
        assert_eq!(m.view("5m")["series"].as_array().unwrap().len(), 4);
        assert!(m.watched());
    }

    #[test]
    fn breakdown_by_process_tree() {
        // 用真的进程表：起一个子进程（客户端的子进程 → 辅助进程），再把它当成一个终端的起点
        // 非 Windows 上用一个改了名的 sleep：辅助进程按程序名合并，别的测试同时起的 sleep 不会混进来
        let tmp = tempfile::tempdir().unwrap();
        let program = if cfg!(windows) {
            std::path::PathBuf::from("cmd")
        } else {
            let p = tmp.path().join(format!("llmet{}", std::process::id() % 100_000));
            std::fs::copy(which_sleep(), &p).unwrap();
            p
        };
        let mut child = std::process::Command::new(&program).args(if cfg!(windows) { vec!["/C", "ping -n 5 127.0.0.1 >NUL"] } else { vec!["5"] }).spawn().unwrap();
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_cpu().with_memory());
        let procs: HashMap<u32, &Process> = sys.processes().iter().filter(|(_, p)| p.thread_kind().is_none()).map(|(pid, p)| (pid.as_u32(), p)).collect();
        let me = std::process::id();
        let l = breakdown(&procs, &Roots::default(), me, 1.0);
        assert_eq!(l.main.procs, 1);
        let child_name = procs[&child.id()].name().to_string_lossy().into_owned();
        let unique = !cfg!(windows);
        assert!(l.helpers.procs >= 1 && l.helper_groups.iter().any(|g| g.name == child_name));
        let roots = Roots {
            sessions: vec![
                SessionRoots { id: "a".into(), name: "A".into(), launch: "shell".into(), roots: vec![child.id()] },
                SessionRoots { id: "b".into(), name: "B".into(), launch: "claude".into(), roots: vec![] },
                // 客户端是在这个终端里启动的：客户端自己不算进去
                SessionRoots { id: "c".into(), name: "C".into(), launch: "shell".into(), roots: vec![me] },
            ],
            ..Default::default()
        };
        let l = breakdown(&procs, &roots, me, 1.0);
        assert_eq!(l.sessions[0].id, "a");
        assert!(l.sessions[0].running && l.sessions[0].usage.procs >= 1 && l.sessions[0].top.is_some());
        assert!(l.sessions.iter().all(|s| s.id == "a" || !s.running), "没进程的、根是客户端自己的都不算在跑");
        assert!(!unique || !l.helper_groups.iter().any(|g| g.name == child_name), "算进终端的不再算辅助进程");
        // Codex 后台命令：算进它的终端，也单独列出
        let bg = Roots {
            sessions: vec![SessionRoots { id: "b".into(), name: "B".into(), launch: "codex".into(), roots: vec![] }],
            codex_bg: vec![BgRoot { session: "b".into(), process_id: "7".into(), command: "npm start".into(), pid: Some(child.id()) }],
            ..Default::default()
        };
        let l = breakdown(&procs, &bg, me, 1.0);
        assert!(l.sessions[0].running && l.sessions[0].usage.procs >= 1);
        assert_eq!(l.background.len(), 1);
        assert_eq!((l.background[0].process_id.as_str(), l.background[0].usage.procs), ("7", l.sessions[0].usage.procs));
        assert!(!unique || !l.helper_groups.iter().any(|g| g.name == child_name), "后台命令不算辅助进程");
        assert_eq!(l.total.procs, l.main.procs + l.helpers.procs + l.sessions.iter().map(|s| s.usage.procs).sum::<u32>());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(not(windows))]
    fn which_sleep() -> std::path::PathBuf {
        ["/bin/sleep", "/usr/bin/sleep"].into_iter().map(std::path::PathBuf::from).find(|p| p.is_file()).expect("sleep")
    }
    #[cfg(windows)]
    fn which_sleep() -> std::path::PathBuf {
        unreachable!()
    }

    #[test]
    fn samples_this_machine() {
        let mut s = Sampler::new();
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let n = s.sample(true, None);
        assert!(n.mem_total > 0 && !n.cores.is_empty());
        assert!(n.cpu >= 0.0 && n.cpu <= 100.0);
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let n = s.sample(true, Some(&Roots::default()));
        assert!(n.processes.is_some_and(|p| p > 0) && !n.top.is_empty());
        let l = n.looklook.expect("有人看时算看看自己的占用");
        assert!(l.main.procs == 1 && l.main.mem > 0 && l.total.mem >= l.main.mem);
    }
}
