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
    /// 宠物在窗口里的框 [x, y, w, h]（CSS 像素）；None = 没显示
    pub hit: Option<[f64; 4]>,
    /// 主窗口正在看的对话（实例 id）
    pub focus: Option<String>,
    /// 宠物正被拖着：鼠标移出框也不能穿透
    pub grab: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static WANTED: Mutex<Option<watch::Sender<bool>>> = Mutex::new(None);
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn get() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// 页面报上来的：只改带了的字段
pub fn update(f: impl FnOnce(&mut State)) -> State {
    let mut g = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(State::default);
    f(s);
    s.clone()
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

/// 桌面应用开着宠物窗口（主窗口的对话页据此不在页面里再放一只）
#[allow(dead_code)]
pub fn set_running(on: bool) {
    RUNNING.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn running() -> bool {
    RUNNING.load(std::sync::atomic::Ordering::Relaxed)
}
