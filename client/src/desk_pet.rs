//! 桌面宠物（桌面应用）：宠物住在一个铺满屏幕的透明置顶窗口里，应用最小化、主窗口关掉也在。
//!
//! 页面不能调 Tauri（管理台是外部网址，没开 IPC），所以和桌面这边隔着本机接口说话：
//! - 宠物窗口的页面（`/pet-desk`）每次宠物动了报一下它在窗口里占的框（`hit`），桌面这边按鼠标在不在框里
//!   切换“鼠标穿透”——框外的点击落到下面的窗口上；
//! - 主窗口的对话页报“正在看哪个对话”（`focus`），宠物窗口跟着它；
//! - 设置里开关桌面宠物（`wanted`），桌面这边按它开、关窗口。
//!
//! 命令行版（Linux、没开桌面应用）这些状态照样有，只是没人用。

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    /// 宠物在它所在窗口里的框 [x, y, w, h]（CSS 像素）；None = 没显示
    pub hit: Option<[f64; 4]>,
    /// 主窗口正在看的对话（实例 id）
    pub focus: Option<String>,
    /// 宠物正被拖着：鼠标移出框也不能穿透
    pub grab: bool,
    /// 每块屏幕一个宠物窗口，宠物在哪块（`screens` 的下标）
    pub owner: usize,
    /// 刚换到这块屏幕时放在哪：[x, 离底边 y]（CSS 像素）和往哪边走进来；换一次 `seq` + 1
    pub entry: Option<Entry>,
    /// 各块屏幕的工作区（物理像素）和缩放
    pub screens: Vec<Screen>,
    /// 网页里的宠物（同一台电脑上、在前台）：和桌面宠物互动，见 [`Peer`]
    pub peer: Option<Peer>,
    pub chat: Option<Chat>,
    /// 主窗口转过来的会话提醒（开了“宠物来报信”）：桌面宠物说出来，点气泡打开那个会话
    pub note: Option<Note>,
    /// 点了桌面宠物的提醒气泡：主窗口打开这个会话
    pub open: Option<Note>,
    /// 变了就 + 1（宠物窗口长轮询它）
    pub seq: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Entry {
    pub x: f64,
    pub y: f64,
    /// `left` / `right`：从这边进来，往里走；None = 拖过来放下的
    pub walk: Option<String>,
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Screen {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    pub scale: f64,
}

/// 网页里的宠物：在屏幕上的位置（物理像素，按浏览器窗口位置估的）、名字、最后报的时间（毫秒）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Peer {
    pub x: f64,
    pub y: f64,
    pub name: String,
    pub at: i64,
}

/// 两只宠物说的话：`from` = `web` / `desk`，`key` 是台词键（各自按自己的人设挑句子），`reply` = 这是在接话（对方不用再接）
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Chat {
    pub from: String,
    pub key: String,
    pub reply: bool,
    pub seq: u64,
}

/// 会话提醒：`kind` = `done` / `failed` / `attn`，`id` 是会话（实例）id
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Note {
    pub kind: String,
    pub title: String,
    pub id: String,
    pub seq: u64,
    /// 什么时候（毫秒）：主窗口刚开起来时只认最近几秒的
    pub at: i64,
}

impl Screen {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// 宠物从 `from` 块屏幕的窗口里出去了：(x, y) 是那个窗口里的 CSS 像素（y 从顶上算），可以在窗口外面。
/// 换算成物理像素，落在哪块屏幕上就交给哪块；`side` 是走出去的那边（走过去的贴着对面的边进来）。
/// 返回 `(新屏幕, 新窗口里的 x, 离底边 y)`；外面没有屏幕时 None。
pub fn handoff(screens: &[Screen], from: usize, x: f64, y: f64, side: Option<&str>) -> Option<(usize, f64, f64)> {
    let a = screens.get(from)?;
    let k = if a.scale > 0.0 { a.scale } else { 1.0 };
    let (px, py) = (a.x + x * k, a.y + y * k);
    let (i, b) = screens.iter().enumerate().find(|(i, s)| *i != from && s.contains(px, py)).or_else(|| {
        // 走出去时脚底那一行可能正好不在旁边屏幕的范围里（两块屏幕上下错开）：找那一侧上下最近的
        let side = side?;
        screens
            .iter()
            .enumerate()
            .filter(|(i, s)| *i != from && if side == "right" { (s.x - (a.x + a.w)).abs() < 8.0 } else { (s.x + s.w - a.x).abs() < 8.0 })
            .min_by(|(_, p), (_, q)| (p.y + p.h / 2.0 - py).abs().total_cmp(&(q.y + q.h / 2.0 - py).abs()))
    })?;
    let kb = if b.scale > 0.0 { b.scale } else { 1.0 };
    let (w, h) = (b.w / kb, b.h / kb);
    let nx = match side {
        Some("right") => 0.0,
        Some("left") => w,
        _ => ((px - b.x) / kb).clamp(0.0, w),
    };
    let ny = (h - (py - b.y) / kb).clamp(0.0, h);
    Some((i, nx, ny))
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static WANTED: Mutex<Option<watch::Sender<bool>>> = Mutex::new(None);
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn get() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// 页面报上来的：只改带了的字段；变了就通知在等的宠物窗口
pub fn update(f: impl FnOnce(&mut State)) -> State {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(State::default);
    // 框、拖着没有一直在变（走动、拖拽时每帧都报）：不算“变了”，不叫醒等着的窗口
    let quiet = |x: &State| State { hit: None, grab: false, peer: x.peer.as_ref().map(|p| Peer { at: 0, ..p.clone() }), ..x.clone() };
    let before = quiet(s);
    f(s);
    if quiet(s) != before {
        s.seq += 1;
        let seq = s.seq;
        drop(g);
        seq_tx().send_replace(seq);
        return get();
    }
    s.clone()
}

static SEQ: std::sync::OnceLock<watch::Sender<u64>> = std::sync::OnceLock::new();

fn seq_tx() -> &'static watch::Sender<u64> {
    SEQ.get_or_init(|| watch::channel(0).0)
}

/// 等状态变（`seq` 不再是 `since`），最多 `max`
pub async fn wait_change(since: u64, max: std::time::Duration) -> State {
    let mut rx = seq_tx().subscribe();
    let _ = tokio::time::timeout(max, rx.wait_for(|v| *v != since)).await;
    get()
}

fn sender() -> watch::Sender<bool> {
    WANTED.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| watch::channel(false).0).clone()
}

/// 设置里桌面宠物开没开（启动时、改设置时调）
pub fn set_wanted(on: bool) {
    sender().send_replace(on);
}

/// 桌面应用订阅：开关变了就开、关宠物窗口
#[allow(dead_code)]
pub fn wanted() -> watch::Receiver<bool> {
    sender().subscribe()
}

static OPEN: std::sync::OnceLock<watch::Sender<u64>> = std::sync::OnceLock::new();

fn open_tx() -> &'static watch::Sender<u64> {
    OPEN.get_or_init(|| watch::channel(0).0)
}

/// 点了桌面宠物的提醒气泡（桌面应用订阅它，把主窗口叫出来）
pub fn opened() {
    open_tx().send_modify(|n| *n += 1);
}

#[allow(dead_code)]
pub fn open_requests() -> watch::Receiver<u64> {
    open_tx().subscribe()
}

/// 桌面应用开着宠物窗口（主窗口的对话页据此不在页面里再放一只）
#[allow(dead_code)]
pub fn set_running(on: bool) {
    RUNNING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn running() -> bool {
    RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_between_screens() {
        // 左边 1920×1080（缩放 1），右边 2560×1440 的 Retina（缩放 2，逻辑 1280×720），底边对齐
        let s = [Screen { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0, scale: 1.0 }, Screen { x: 1920.0, y: -360.0, w: 2560.0, h: 1440.0, scale: 2.0 }];
        // 走出右边：脚在 y=1000 → 物理 (1925, 1000) 在右屏里，从左边进来
        let (i, x, y) = handoff(&s, 0, 1925.0, 1000.0, Some("right")).unwrap();
        assert_eq!(i, 1);
        assert_eq!(x, 0.0);
        assert!((y - (720.0 - (1000.0 + 360.0) / 2.0)).abs() < 0.01);
        // 拖到右屏中间松手
        let (i, x, _) = handoff(&s, 0, 1920.0 + 640.0, 500.0, None).unwrap();
        assert_eq!((i, x), (1, 320.0));
        // 从右屏往左走出去
        let (i, x, _) = handoff(&s, 1, -1.0, 600.0, Some("left")).unwrap();
        assert_eq!((i, x), (0, 1920.0));
        // 左屏左边外面没有屏幕
        assert!(handoff(&s, 0, -5.0, 500.0, Some("left")).is_none());
        // 上下错开、脚底那行不在右屏范围里：找那一侧最近的
        let s2 = [Screen { x: 0.0, y: 0.0, w: 100.0, h: 100.0, scale: 1.0 }, Screen { x: 100.0, y: -200.0, w: 100.0, h: 150.0, scale: 1.0 }];
        assert_eq!(handoff(&s2, 0, 101.0, 90.0, Some("right")).map(|r| r.0), Some(1));
    }
}
