//! 桌面应用（Windows / macOS）：Tauri 窗口直接显示管理台，系统托盘常驻。
//!
//! 启动后打开应用窗口（不再打开浏览器）；关掉窗口只是隐藏，服务继续在后台运行，
//! 点托盘图标（macOS 也可以点程序坞图标）重新显示。托盘菜单：打开看看 / 启动服务 /
//! 停止服务 / 开机启动 / 退出。开机自启（`run --no-browser`）只启动服务、不弹窗口。
//!
//! macOS：只有窗口显示时程序坞里才有图标（就是应用图标），窗口隐藏后切换成
//! `Accessory` 模式只留菜单栏图标——以前裸二进制跑 tao 事件循环会在程序坞多出一个
//! 通用的“exec”图标。
//!
//! Linux 没有这个文件（整个 `#![cfg(...)]` 关掉），照常靠 systemd 用户服务无界面运行，
//! 也不需要 GTK / WebKitGTK（Cargo.toml 里 tauri 放在按平台生效的依赖里）。
//!
//! 架构：Tauri 的事件循环必须跑在主线程，tokio 运行时挪到后台线程跑服务（`supervisor`），
//! 并交给 Tauri 共用（`tauri::async_runtime::set`）。
#![cfg(any(windows, target_os = "macos"))]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
#[cfg(windows)]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent, Wry};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MAIN: &str = "main";
/// 桌面宠物窗口（`desk_pet`）：每块屏幕一个（`pet-0`、`pet-1`……），铺满那块屏幕的工作区、透明、置顶，
/// 只有宠物身上能点；宠物在哪块屏幕由 `desk_pet` 的 `owner` 定，走过、拖过屏幕边就换一个窗口显示
const PET: &str = "pet";

fn pet_label(i: usize) -> String {
    format!("{PET}-{i}")
}

fn pet_windows(app: &AppHandle) -> Vec<(usize, tauri::WebviewWindow)> {
    let mut v: Vec<(usize, tauri::WebviewWindow)> =
        app.webview_windows().into_iter().filter_map(|(k, w)| k.strip_prefix("pet-").and_then(|n| n.parse().ok()).map(|i| (i, w))).collect();
    v.sort_by_key(|(i, _)| *i);
    v
}

/// 托盘菜单、窗口标题是原生控件，走不了网页那套 i18n：`src-ui/shared/i18n.js` 选的语言只存在浏览器的
/// `localStorage`（键 `ll_locale`）里，主进程完全拿不到。这里退而求其次，用系统语言猜一下，
/// 猜不到时跟网页端的默认值保持一致，用中文（`i18n.js` 里 `fallbackLng: 'zh-CN'`，
/// `browserLocale()` 也是“不是以 en 开头就当中文”）。
fn tr(zh: &'static str, en: &'static str) -> &'static str {
    static EN: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *EN.get_or_init(os_locale_is_english) {
        en
    } else {
        zh
    }
}

#[cfg(windows)]
fn os_locale_is_english() -> bool {
    // Windows 上没有 Unix 那套 LANG/LC_ALL 惯例；这里不为了猜语言再加一个注册表/API 依赖，
    // 只在设置了 LANG（比如 MSYS2/WSL 混用环境）时参考一下，猜不到就落回中文，跟网页端默认值一致。
    std::env::var("LANG").ok().map(|l| l.to_ascii_lowercase().starts_with("en")).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn os_locale_is_english() -> bool {
    if let Ok(out) = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output() {
        let s = String::from_utf8_lossy(&out.stdout).to_ascii_lowercase();
        if let Some(first) = s.lines().find(|l| l.contains('"')) {
            return first.contains("\"en");
        }
    }
    std::env::var("LANG").map(|l| l.to_ascii_lowercase().starts_with("en")).unwrap_or(false)
}

enum Cmd {
    Start,
    Stop,
    Exit(std_mpsc::Sender<()>),
}

struct Shell {
    home: Option<PathBuf>,
    listen: Option<SocketAddr>,
    cmd: mpsc::UnboundedSender<Cmd>,
    running: Arc<AtomicBool>,
    /// 当前这次 run() 的管理台地址（服务就绪后才有）。
    url: Arc<std::sync::Mutex<Option<String>>>,
}

/// 供 `main.rs` 调用：接管主线程跑 Tauri 事件循环，退出时直接结束进程。
pub fn run(rt: tokio::runtime::Runtime, home: Option<PathBuf>, listen: Option<SocketAddr>, server: Option<String>, no_browser: bool) -> Result<()> {
    tauri::async_runtime::set(rt.handle().clone());
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let running = Arc::new(AtomicBool::new(true));
    let url = Arc::new(std::sync::Mutex::new(None));
    // 更新后由旧进程拉起的新进程：旧进程还没退出，不能被当成“第二份”而把参数转给它后自己退掉。
    let restarted = std::env::var_os(crate::updater::RESTART_ENV).is_some();
    let shell = Shell { home: home.clone(), listen, cmd: cmd_tx, running: running.clone(), url: url.clone() };

    let mut builder = tauri::Builder::default();
    if !restarted {
        // 再次双击图标（或开始菜单里再点一次）：显示已经在运行的窗口，而不是再启动一份。
        // 登录自启（run --no-browser）撞上已经在运行的应用时什么都不做。
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !argv.iter().any(|a| a == "--no-browser") {
                show_main(app);
            }
        }));
    }
    #[cfg(target_os = "macos")]
    {
        builder = builder.menu(mac_menu);
    }
    let app = builder
        .manage(shell)
        .on_menu_event(on_menu)
        .on_window_event(|window, event| {
            if window.label() != MAIN {
                return;
            }
            match event {
                // 关掉主窗口只是隐藏：服务和托盘继续在后台运行。
                WindowEvent::CloseRequested { api, .. } => {
                    geometry::save_now(window);
                    api.prevent_close();
                    let _ = window.hide();
                    dock(window.app_handle(), false);
                }
                // 记住窗口大小和位置：下次打开、升级后重启都按上次的来
                WindowEvent::Resized(_) | WindowEvent::Moved(_) => geometry::save_later(window),
                _ => {}
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            #[cfg(target_os = "macos")]
            {
                autostart::default_on(home.as_ref());
                disable_app_nap();
            }
            let items = tray(&handle)?;
            app.manage(items.clone());
            let rt_running = running.clone();
            let rt_url = url.clone();
            std::thread::Builder::new().name("looklook-rt".into()).spawn(move || {
                rt.block_on(supervisor(home, listen, server, cmd_rx, rt_running, rt_url));
            })?;
            tauri::async_runtime::spawn(refresh_menu(items, running.clone()));
            tauri::async_runtime::spawn(pet_supervisor(handle.clone()));
            tauri::async_runtime::spawn(exit_on_sigterm(handle.clone()));
            if no_browser {
                dock(&handle, false);
            } else {
                show_main(&handle);
            }
            Ok(())
        })
        .build(tauri::generate_context!())?;
    app.run(|app, event| match event {
        // 窗口都关了不退出（托盘常驻）；只有“退出”菜单（app.exit）才真正退出。
        RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
        RunEvent::Exit => {
            if let Some(w) = app.get_webview_window(MAIN) {
                geometry::save_now(&w.as_ref().window());
            }
            // 退出前把服务停干净（ttyd/实例收尾、关通道），最多等 10 秒。
            let (tx, rx) = std_mpsc::channel();
            if app.state::<Shell>().cmd.send(Cmd::Exit(tx)).is_ok() {
                let _ = rx.recv_timeout(Duration::from_secs(10));
            }
            // 不再回到 Tauri / AppKit 去调 exit()：后台线程还在时析构全局对象会崩溃（见 util::exit_now）
            tracing::info!("看看已退出");
            crate::util::exit_now(0);
        }
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        _ => {}
    });
    Ok(())
}

/// 等这次服务就绪；它没有启动（例如同一数据目录已有一份命令行启动的客户端）时，用那一份的地址。
async fn wait_url(app: &AppHandle) -> Option<String> {
    let shell = app.state::<Shell>();
    for _ in 0..120 {
        if let Some(u) = shell.url.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Some(u);
        }
        if !shell.running.load(Ordering::Relaxed) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let fallback = shell.listen.unwrap_or(crate::listen::DEFAULT);
    if crate::already_running(&shell.home, fallback).await {
        return crate::local_url(&shell.home, fallback).ok();
    }
    tracing::error!("管理台没有启动，查看日志 logs/looklook.log");
    None
}

/// 点了桌面宠物的提醒气泡：把主窗口叫出来（打开哪个会话由主窗口自己看 `/desktop/pet` 的 `open`）
async fn pet_open_watch(app: AppHandle) {
    let mut rx = crate::desk_pet::open_requests();
    while rx.changed().await.is_ok() {
        let h = app.clone();
        let _ = app.run_on_main_thread(move || show_main(&h));
    }
}

/// 桌面宠物：设置里开着就开窗口，关了就关；窗口在时按鼠标位置切换“鼠标穿透”。
async fn pet_supervisor(app: AppHandle) {
    tauri::async_runtime::spawn(pet_open_watch(app.clone()));
    let mut rx = crate::desk_pet::wanted();
    let mut poll: Option<CancellationToken> = None;
    loop {
        let on = *rx.borrow_and_update();
        if on && pet_windows(&app).is_empty() {
            if let Some(url) = wait_url(&app).await.and_then(|u| u.parse::<Url>().ok()).and_then(|u| u.join("/pet-desk").ok()) {
                let h = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Err(e) = pet_windows_open(&h, url) {
                        tracing::warn!(error = %e, "桌面宠物窗口没开起来");
                    }
                });
                let stop = CancellationToken::new();
                tauri::async_runtime::spawn(pet_hover(app.clone(), stop.clone()));
                poll = Some(stop);
                crate::desk_pet::set_running(true);
            }
        } else if !on {
            if let Some(stop) = poll.take() {
                stop.cancel();
            }
            for (_, w) in pet_windows(&app) {
                let _ = w.close();
            }
            crate::desk_pet::set_running(false);
        }
        if rx.changed().await.is_err() {
            break;
        }
    }
}

/// 每块屏幕开一个宠物窗口；屏幕的位置、大小、缩放记进 `desk_pet`（宠物换屏幕时换算坐标用）
fn pet_windows_open(app: &AppHandle, url: Url) -> tauri::Result<()> {
    let mut monitors = app.available_monitors().unwrap_or_default();
    if monitors.is_empty() {
        monitors.extend(app.primary_monitor().ok().flatten());
    }
    // 主屏排第一（宠物第一次出现在主屏上）
    if let Ok(Some(p)) = app.primary_monitor() {
        if let Some(i) = monitors.iter().position(|m| m.position() == p.position()) {
            monitors.swap(0, i);
        }
    }
    let screens: Vec<crate::desk_pet::Screen> = monitors
        .iter()
        .map(|m| {
            let a = m.work_area();
            crate::desk_pet::Screen { x: a.position.x as f64, y: a.position.y as f64, w: a.size.width as f64, h: a.size.height as f64, scale: m.scale_factor() }
        })
        .collect();
    let n = screens.len();
    crate::desk_pet::update(|s| {
        s.screens = screens;
        if s.owner >= n {
            s.owner = 0;
            s.entry = None;
        }
    });
    for (i, m) in monitors.iter().enumerate() {
        let mut u = url.clone();
        u.set_query(Some(&format!("screen={i}")));
        let w = WebviewWindowBuilder::new(app, pet_label(i), WebviewUrl::External(u))
            .initialization_script("window.__LOOKLOOK_DESKTOP__ = true; window.__LOOKLOOK_PET_DESK__ = true;")
            .title(tr("看看宠物", "Looklook pet"))
            .transparent(true)
            .decorations(false)
            .shadow(false)
            .resizable(false)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .focused(false)
            // 点、拖宠物不抢焦点：Windows 上是 WS_EX_NOACTIVATE，Linux 上不接受焦点；macOS 还要改成不激活的面板（见 pet_panel）
            .focusable(false)
            .accept_first_mouse(true)
            .visible(false)
            .build()?;
        #[cfg(target_os = "macos")]
        pet_panel(&w);
        // 铺满这块屏幕的工作区（不盖任务栏 / 程序坞）
        let a = m.work_area();
        w.set_position(a.position)?;
        w.set_size(a.size)?;
        w.set_ignore_cursor_events(true)?;
        w.show()?;
    }
    Ok(())
}

/// macOS：点普通窗口会把整个应用激活（主窗口也跟着跑到前面），只有带 NonactivatingPanel 的 NSPanel 不会。
/// 把宠物窗口的类换成 NSPanel 的子类（不能成为焦点 / 主窗口），再加上这个样式（tauri-nspanel 也是这么做的）。
#[cfg(target_os = "macos")]
fn pet_panel(w: &tauri::WebviewWindow) {
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{msg_send, sel};
    struct Cls(&'static AnyClass);
    // SAFETY: 类对象注册后不变，可以跨线程共享
    unsafe impl Send for Cls {}
    unsafe impl Sync for Cls {}
    static CLS: std::sync::OnceLock<Option<Cls>> = std::sync::OnceLock::new();
    extern "C-unwind" fn no(_: &AnyObject, _: Sel) -> Bool {
        Bool::NO
    }
    let cls = CLS.get_or_init(|| {
        let mut b = ClassBuilder::new(c"LooklookPetPanel", AnyClass::get(c"NSPanel")?)?;
        // SAFETY: 签名和 NSWindow 的 canBecomeKeyWindow / canBecomeMainWindow 一致（无参数，返回 BOOL）
        unsafe {
            b.add_method(sel!(canBecomeKeyWindow), no as extern "C-unwind" fn(_, _) -> _);
            b.add_method(sel!(canBecomeMainWindow), no as extern "C-unwind" fn(_, _) -> _);
        }
        Some(Cls(b.register()))
    });
    let (Some(Cls(cls)), Ok(ptr)) = (cls, w.ns_window()) else {
        tracing::warn!("桌面宠物窗口没能改成面板，点它会激活看看");
        return;
    };
    let obj = ptr.cast::<AnyObject>();
    const NONACTIVATING_PANEL: usize = 1 << 7;
    // SAFETY: obj 是活着的 NSWindow（在主线程上），NSPanel 是 NSWindow 的子类、没有多出来的实例变量
    unsafe {
        objc2::ffi::object_setClass(obj, *cls);
        let mask: usize = msg_send![obj, styleMask];
        let _: () = msg_send![obj, setStyleMask: mask | NONACTIVATING_PANEL];
        // 面板默认在应用失去激活时藏起来；宠物要一直在
        let _: () = msg_send![obj, setHidesOnDeactivate: false];
        let _: () = msg_send![obj, setBecomesKeyOnlyIfNeeded: true];
        // 窗口建好以后再改成 NonactivatingPanel，有的系统版本上窗口服务器那边没跟着变（点了照样激活应用）：私有接口补一下
        let sel = sel!(_setPreventsActivation:);
        let can: bool = msg_send![obj, respondsToSelector: sel];
        if can {
            let _: () = msg_send![obj, _setPreventsActivation: true];
        }
    }
}

/// 每 40 ms 看一次鼠标在不在宠物身上：在就让宠物所在的窗口接鼠标，不在就穿透到下面的窗口；别的屏幕上的宠物窗口一直穿透。
/// 开了“不挡操作”：只有按着抓宠物的键（`desk_pet::grab_key`）、或头上有能点的提醒气泡时才接。
/// 鼠标在宠物身上（能抓）就告诉页面（`hot`），半透明的宠物变实
async fn pet_hover(app: AppHandle, stop: CancellationToken) {
    let through = crate::desk_pet::through();
    let mut ignore: std::collections::HashMap<usize, bool> = std::collections::HashMap::new();
    loop {
        tokio::select! {
            _ = stop.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_millis(40)) => {}
        }
        let st = crate::desk_pet::get();
        let thru = *through.borrow();
        let key = thru && crate::desk_pet::grab_key_down();
        let mut hot = false;
        for (i, w) in pet_windows(&app) {
            let on_pet = i == st.owner
                && match (st.hit, app.cursor_position(), w.outer_position(), w.scale_factor()) {
                    (Some([x, y, wd, ht]), Ok(c), Ok(o), Ok(k)) if k > 0.0 => {
                        let (cx, cy) = ((c.x - o.x as f64) / k, (c.y - o.y as f64) / k);
                        cx >= x && cx <= x + wd && cy >= y && cy <= y + ht
                    }
                    _ => false,
                };
            let grabbable = on_pet && (!thru || key);
            let inside = i == st.owner && (st.grab || grabbable || (on_pet && st.tap));
            if i == st.owner {
                hot = st.grab || grabbable;
            }
            if ignore.get(&i).copied() != Some(!inside) {
                ignore.insert(i, !inside);
                let _ = w.set_ignore_cursor_events(!inside);
            }
        }
        if hot != st.hot {
            crate::desk_pet::update(|s| s.hot = hot);
        }
    }
}

/// 显示（没有就创建）主窗口并置前。
fn show_main(app: &AppHandle) {
    dock(app, true);
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(url) = wait_url(&app).await else { return };
        let Ok(url) = url.parse::<Url>() else { return };
        let handle = app.clone();
        // 创建窗口要在主线程上做。
        let _ = app.run_on_main_thread(move || {
            let saved = geometry::load(&handle);
            let mut b = window(&handle, MAIN, url.clone(), url).min_inner_size(420.0, 480.0);
            b = match &saved {
                Some(g) => {
                    let b = b.inner_size(g.width, g.height);
                    match g.position {
                        Some((x, y)) => b.position(x, y),
                        None => b.center(),
                    }
                }
                None => b.inner_size(1280.0, 820.0).center(),
            };
            match b.build() {
                Ok(w) => {
                    if saved.as_ref().is_some_and(|g| g.maximized) {
                        let _ = w.maximize();
                    }
                    let _ = w.set_focus();
                }
                Err(e) => tracing::error!(error = %e, "无法创建窗口"),
            }
        });
    });
}

/// 管理台窗口。网页里 `window.open` 的本机页面（终端“新标签页打开”等）开成新的应用窗口，
/// 其它网址（看看网站、会员页、授权页）交给系统浏览器。
///
/// `console`：本机管理台地址；`url`：窗口一开始加载的地址。网页新开的窗口先加载 about:blank，由 WebKit / WebView2
/// 把请求的页面载入返回的窗口（Tauri 文档的做法）。自己再加载一次目标地址会有两次导航，前一次被取消后
/// macOS 上 WKWebView 的地址短暂为空，wry 读它时 panic，应用直接崩溃（用户报告：打开 DSH、终端“新窗口打开”）。
fn window<'a>(app: &'a AppHandle, label: &str, console: Url, url: Url) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    let handle = app.clone();
    WebviewWindowBuilder::new(app, label, WebviewUrl::External(url))
        // 管理台据此知道自己在桌面应用里（例如 DSH 改由系统浏览器打开）
        .initialization_script("window.__LOOKLOOK_DESKTOP__ = true;")
        .title(tr("看看", "Looklook"))
        // macOS：窗口在后台时点一下就生效（例如直接切到侧栏里的会话），不用先点一下把窗口叫到前面
        .accept_first_mouse(true)
        .on_document_title_changed(|w, title| {
            let _ = w.set_title(&title);
        })
        .on_new_window(move |target, features| {
            let same = target.as_str() == "about:blank" || (target.origin() == console.origin());
            if !same {
                let t = target.to_string();
                std::thread::spawn(move || crate::util::open_url(&t));
                return NewWindowResponse::Deny;
            }
            // macOS：交回 Tauri 新建的网页视图（上面那种做法）在 1.19 上仍会让 WebKit 抛 Objective-C 异常，应用直接退出
            // （问题报告里只有一条 “panic in a function that cannot unwind”，没有 Rust 自己的 panic 信息）。
            // 先开空白页再跳转的（DSH、未运行的终端）交给 wry 按 WebKit 的要求自己建窗口，页面拿得到窗口句柄；
            // 直接打开本机网址的，拒绝这次请求，等回调结束后再照常新建应用窗口。
            if cfg!(target_os = "macos") {
                if target.as_str() == "about:blank" {
                    return NewWindowResponse::Allow;
                }
                let (handle, console) = (handle.clone(), console.clone());
                std::thread::spawn(move || {
                    let h = handle.clone();
                    let _ = handle.run_on_main_thread(move || {
                        if let Err(e) = window(&h, &next_label(), console, target).inner_size(1100.0, 720.0).build() {
                            tracing::warn!(error = %e, "无法打开新窗口");
                        }
                    });
                });
                return NewWindowResponse::Deny;
            }
            let blank = Url::parse("about:blank").expect("about:blank");
            match window(&handle, &next_label(), console.clone(), blank).window_features(features).inner_size(1100.0, 720.0).build() {
                Ok(window) => NewWindowResponse::Create { window },
                Err(e) => {
                    tracing::warn!(error = %e, "无法打开新窗口");
                    NewWindowResponse::Deny
                }
            }
        })
}

/// 新开应用窗口的标签：w1、w2……
fn next_label() -> String {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    format!("w{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// macOS：窗口显示时程序坞里有应用图标，全部隐藏后只留菜单栏图标。
fn dock(app: &AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        let visible = visible || app.webview_windows().iter().any(|(l, w)| l != MAIN && !l.starts_with("pet-") && w.is_visible().unwrap_or(false));
        let policy = if visible { tauri::ActivationPolicy::Regular } else { tauri::ActivationPolicy::Accessory };
        let _ = app.set_activation_policy(policy);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
}

#[derive(Clone)]
struct Items {
    start: MenuItem<Wry>,
    stop: MenuItem<Wry>,
    auto: CheckMenuItem<Wry>,
    /// 桌面宠物开关、不挡操作（和宠物设置同步）：宠物不挡操作又抓不到时（Linux 上没有抓宠物的键）从这里关
    pet: CheckMenuItem<Wry>,
    pet_through: CheckMenuItem<Wry>,
}

fn tray(app: &AppHandle) -> tauri::Result<Items> {
    let open = MenuItem::with_id(app, "open", tr("打开看看", "Open Looklook"), true, None::<&str>)?;
    let start = MenuItem::with_id(app, "start", tr("启动服务", "Start service"), false, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", tr("停止服务", "Stop service"), true, None::<&str>)?;
    let auto = CheckMenuItem::with_id(app, "autostart", tr("开机启动", "Launch at startup"), true, autostart::is_enabled(), None::<&str>)?;
    let pet = CheckMenuItem::with_id(app, "pet", tr("桌面宠物", "Desktop pet"), true, *crate::desk_pet::wanted().borrow(), None::<&str>)?;
    let pet_through =
        CheckMenuItem::with_id(app, "pet_through", tr("宠物不挡操作", "Pet stays out of the way"), true, *crate::desk_pet::through().borrow(), None::<&str>)?;
    let exit = MenuItem::with_id(app, "exit", tr("退出", "Exit"), true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &start, &stop, &sep1, &pet, &pet_through, &sep2, &auto, &sep3, &exit])?;
    let mut tray = TrayIconBuilder::with_id("main").menu(&menu).tooltip(tr("看看客户端", "Looklook client"));
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    // Windows 习惯：左键点图标打开窗口，右键出菜单；macOS 点菜单栏图标出菜单。
    #[cfg(windows)]
    {
        tray = tray.show_menu_on_left_click(false).on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    }
    tray.build(app)?;
    tauri::async_runtime::spawn(pet_menu_sync(pet.clone(), pet_through.clone()));
    Ok(Items { start, stop, auto, pet, pet_through })
}

/// 设置页里改了桌面宠物、不挡操作：托盘菜单的勾跟着变
async fn pet_menu_sync(pet: CheckMenuItem<Wry>, through: CheckMenuItem<Wry>) {
    let (mut a, mut b) = (crate::desk_pet::wanted(), crate::desk_pet::through());
    loop {
        let _ = pet.set_checked(*a.borrow_and_update());
        let _ = through.set_checked(*b.borrow_and_update());
        tokio::select! {
            r = a.changed() => {
                if r.is_err() {
                    return;
                }
            }
            r = b.changed() => {
                if r.is_err() {
                    return;
                }
            }
        }
    }
}

fn on_menu(app: &AppHandle, event: MenuEvent) {
    let shell = app.state::<Shell>();
    match event.id().as_ref() {
        "open" => show_main(app),
        "start" => {
            let _ = shell.cmd.send(Cmd::Start);
        }
        "stop" => {
            let _ = shell.cmd.send(Cmd::Stop);
            // 服务停了页面也用不了：收起主窗口，关掉另开的终端窗口。
            for (label, w) in app.webview_windows() {
                let _ = if label == MAIN { w.hide() } else { w.close() };
            }
            dock(app, false);
        }
        "autostart" => {
            // 勾选框点击后自己已经切换了状态，这里按新状态写入，失败再拨回去。
            let item = &app.state::<Items>().auto;
            let on = item.is_checked().unwrap_or(false);
            if let Err(e) = autostart::set_enabled(on) {
                tracing::warn!(error = %e, "无法修改开机启动设置");
                let _ = item.set_checked(!on);
            }
        }
        "pet" => crate::desk_pet::toggle(crate::desk_pet::Toggle::Desktop(app.state::<Items>().pet.is_checked().unwrap_or(false))),
        "pet_through" => crate::desk_pet::toggle(crate::desk_pet::Toggle::Through(app.state::<Items>().pet_through.is_checked().unwrap_or(false))),
        "exit" | "quit" => app.exit(0),
        id => page_command(app, id),
    }
}

/// macOS 菜单里的常用快捷键（⌘, 设置、⌘N 新会话、⌘W 关标签、⇧⌘W 关窗口、⌘R 重新载入、⌘+ / ⌘- / ⌘0 缩放），作用在当前窗口上。
/// 设置、新会话、关标签交给页面（`ll-menu` 事件，见 App.jsx、Sessions.jsx 的 SessionTabs）；关窗口、页面载入、缩放在这里做。
fn page_command(app: &AppHandle, id: &str) {
    static ZOOM: Mutex<Vec<(String, f64)>> = Mutex::new(Vec::new());
    let windows = app.webview_windows();
    let Some((label, w)) = windows.iter().find(|(_, w)| w.is_focused().unwrap_or(false)).or_else(|| windows.iter().find(|(l, _)| l.as_str() == MAIN)) else { return };
    let zoom = |f: &dyn Fn(f64) -> f64| {
        let mut z = ZOOM.lock().unwrap_or_else(|e| e.into_inner());
        let cur = z.iter().find(|(l, _)| l == label).map_or(1.0, |(_, v)| *v);
        let next = (f(cur) * 10.0).round() / 10.0;
        let next = next.clamp(0.5, 3.0);
        z.retain(|(l, _)| l != label);
        z.push((label.clone(), next));
        let _ = w.set_zoom(next);
    };
    match id {
        "settings" | "new_session" | "close_tab" => {
            let _ = w.eval(format!("window.dispatchEvent(new CustomEvent('ll-menu', {{ detail: '{id}' }}))"));
        }
        "close_window" => {
            let _ = w.close();
        }
        "reload" => {
            let _ = w.eval("location.reload()");
        }
        "zoom_in" => zoom(&|z| z + 0.1),
        "zoom_out" => zoom(&|z| z - 0.1),
        "zoom_reset" => zoom(&|_| 1.0),
        _ => {}
    }
}

/// 注销、关机时 launchd 先给登录项发 SIGTERM：整个应用退出（和菜单“退出”一样），
/// 而不是只停掉服务、托盘还留着，等系统超时后强行结束。
async fn exit_on_sigterm(app: AppHandle) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let Ok(mut term) = signal(SignalKind::terminate()) else { return };
        if term.recv().await.is_some() {
            tracing::info!("收到 SIGTERM，退出");
            app.exit(0);
        }
    }
    #[cfg(not(unix))]
    let _ = app;
}

/// 菜单项按服务状态置灰：运行中只能“停止服务”，已停止只能“启动服务”。
async fn refresh_menu(items: Items, running: Arc<AtomicBool>) {
    let mut shown = None;
    loop {
        let now = running.load(Ordering::Relaxed);
        if shown != Some(now) {
            shown = Some(now);
            let _ = items.start.set_enabled(!now);
            let _ = items.stop.set_enabled(now);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// 关掉 App Nap。看看平时只在托盘里（主窗口隐藏），macOS 会把它当成“看不见的应用”节流：定时器被合并、推迟，
/// 后台网络也降级。远程入口靠每 5 秒一个的在线心跳判断这台电脑在不在线（超过 16 秒没收到就算离线），
/// 心跳一迟到，从远程地址打开就显示“设备离线”，而这台 Mac 上看看明明开着。
/// 声明一个“用户发起、允许系统空闲睡眠”的活动：不阻止合盖 / 空闲睡眠（睡着了本来就连不上），只是不再被节流。
#[cfg(target_os = "macos")]
fn disable_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
    let reason = NSString::from_str("Looklook keeps your terminals reachable from your phone and other computers");
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(NSActivityOptions::UserInitiatedAllowingIdleSystemSleep, &reason);
    // 活动在令牌释放时结束；整个进程期间都要，所以不释放
    std::mem::forget(activity);
    tracing::info!("已关闭 App Nap");
}

#[cfg(target_os = "macos")]
fn mac_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    use tauri::menu::SubmenuBuilder;
    // 自己的“退出”：走 app.exit 先停服务。编辑菜单不能省，否则网页里 ⌘C / ⌘V 不起作用。
    let quit = MenuItem::with_id(app, "quit", tr("退出看看", "Quit Looklook"), true, Some("CmdOrCtrl+Q"))?;
    // 常用快捷键（见 page_command）
    let settings = MenuItem::with_id(app, "settings", tr("设置…", "Settings…"), true, Some("CmdOrCtrl+,"))?;
    let new_session = MenuItem::with_id(app, "new_session", tr("新会话", "New Session"), true, Some("CmdOrCtrl+N"))?;
    // 不用系统自带的“关闭窗口”：它占着 ⌘W，焦点在终端里也会直接关掉整个窗口
    let close_tab = MenuItem::with_id(app, "close_tab", tr("关闭标签页", "Close Tab"), true, Some("CmdOrCtrl+W"))?;
    let close_window = MenuItem::with_id(app, "close_window", tr("关闭窗口", "Close Window"), true, Some("CmdOrCtrl+Shift+W"))?;
    let reload = MenuItem::with_id(app, "reload", tr("重新载入", "Reload"), true, Some("CmdOrCtrl+R"))?;
    let zoom_in = MenuItem::with_id(app, "zoom_in", tr("放大", "Zoom In"), true, Some("CmdOrCtrl+="))?;
    let zoom_out = MenuItem::with_id(app, "zoom_out", tr("缩小", "Zoom Out"), true, Some("CmdOrCtrl+-"))?;
    let zoom_reset = MenuItem::with_id(app, "zoom_reset", tr("实际大小", "Actual Size"), true, Some("CmdOrCtrl+0"))?;
    let main = SubmenuBuilder::new(app, tr("看看", "Looklook"))
        .about(None)
        .separator()
        .item(&settings)
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;
    let file = SubmenuBuilder::new(app, tr("文件", "File")).item(&new_session).separator().item(&close_tab).item(&close_window).build()?;
    let edit = SubmenuBuilder::new(app, tr("编辑", "Edit")).undo().redo().separator().cut().copy().paste().select_all().build()?;
    let view = SubmenuBuilder::new(app, tr("显示", "View")).item(&reload).separator().item(&zoom_reset).item(&zoom_in).item(&zoom_out).separator().fullscreen().build()?;
    let window = SubmenuBuilder::new(app, tr("窗口", "Window")).minimize().maximize().build()?;
    Menu::with_items(app, &[&main, &file, &edit, &view, &window])
}

/// 托盘事件桥到 tokio：管理“当前是否在跑服务”，Start/Stop 只是创建/取消一个
/// `CancellationToken` 并等 `run()` 的 `JoinHandle` 结束。
async fn supervisor(
    home: Option<PathBuf>,
    listen: Option<SocketAddr>,
    server: Option<String>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    running: Arc<AtomicBool>,
    url: Arc<std::sync::Mutex<Option<String>>>,
) {
    // 窗口代替浏览器，run() 自己不再打开浏览器。
    let mut current = Some(spawn_server(home.clone(), listen, server));
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        // 服务自己退出了（比如端口被占用、启动失败）也要反映到托盘上，这样“启动服务”才会重新可用。
        if current.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            current = None;
        }
        running.store(current.is_some(), Ordering::Relaxed);
        *url.lock().unwrap_or_else(|e| e.into_inner()) = if current.is_some() { crate::ui_url() } else { None };
        let cmd = tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(cmd) => cmd,
                None => break,
            },
            _ = tick.tick() => continue,
        };
        match cmd {
            Cmd::Start => {
                if current.is_none() {
                    current = Some(spawn_server(home.clone(), listen, None));
                }
            }
            Cmd::Stop => stop_current(&mut current).await,
            Cmd::Exit(ack) => {
                stop_current(&mut current).await;
                running.store(false, Ordering::Relaxed);
                let _ = ack.send(());
                return;
            }
        }
    }
}

async fn stop_current(current: &mut Option<(CancellationToken, tokio::task::JoinHandle<()>)>) {
    if let Some((token, handle)) = current.take() {
        token.cancel();
        // 等实例收尾（ttyd/tmux）、避免“停止”卡死太久；超时也继续，进程该退还是退。
        if tokio::time::timeout(Duration::from_secs(10), handle).await.is_err() {
            tracing::warn!("服务停止超时");
        }
    }
}

fn spawn_server(home: Option<PathBuf>, listen: Option<SocketAddr>, server: Option<String>) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let token = CancellationToken::new();
    let child = token.clone();
    let handle = tokio::spawn(async move {
        if let Err(e) = crate::run(home, listen, server, true, child).await {
            tracing::error!(error = %e, "看看服务未能启动");
        }
    });
    (token, handle)
}

/// 开机启动（详细设计任务 7c）：托盘“开机启动”勾选框的读写。已有的安装脚本
/// （`packaging/install-windows.bat` 的“启动”文件夹快捷方式、`packaging/install-macos.sh` 的
/// LaunchAgent）在安装时就配置好了自启动，这里是给用户在托盘里随时开关的运行时开关。
mod autostart {
    #[cfg(windows)]
    mod imp {
        use std::path::PathBuf;

        // 安装包（packaging/nsis-hooks.nsh，旧的 install-windows.bat 也一样）在“启动”文件夹里建
        // “看看.lnk”；这里为了不额外引入 COM 依赖，打开开关时放一个小的 .cmd 启动脚本。
        // 两者任意一个存在就算开着，关掉时两个都删。
        const FILE_NAME: &str = "looklook-tray-autostart.cmd";
        const SHORTCUT: &str = "看看.lnk";

        fn startup() -> Option<PathBuf> {
            dirs::config_dir().map(|d| d.join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Startup"))
        }

        pub fn is_enabled() -> bool {
            startup().is_some_and(|d| d.join(FILE_NAME).is_file() || d.join(SHORTCUT).is_file())
        }

        pub fn set_enabled(on: bool) -> std::io::Result<()> {
            let dir = startup().ok_or_else(|| std::io::Error::other("无法确定“启动”文件夹的位置"))?;
            let path = dir.join(FILE_NAME);
            if !on {
                match std::fs::remove_file(dir.join(SHORTCUT)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
            if on && is_enabled() {
                return Ok(());
            }
            if on {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let exe = std::env::current_exe()?;
                let script = format!("@echo off\r\nstart \"\" \"{}\" run --no-browser\r\n", exe.display());
                std::fs::write(&path, script)
            } else {
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(e) => Err(e),
                }
            }
        }
    }
    #[cfg(windows)]
    pub use imp::{is_enabled, set_enabled};

    #[cfg(target_os = "macos")]
    mod imp {
        use std::path::PathBuf;

        // 和 install-macos.sh 用同一个 Label（`com.looklook.client`）与同一个 plist 文件：
        // 托盘开关操作的就是安装脚本创建的那个登录项，不会出现两份自启动。
        const LABEL: &str = "com.looklook.client";

        fn plist_path() -> Option<PathBuf> {
            dirs::home_dir().map(|h| h.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
        }

        pub fn is_enabled() -> bool {
            plist_path().map(|p| p.is_file()).unwrap_or(false)
        }

        pub fn set_enabled(on: bool) -> std::io::Result<()> {
            let path = plist_path().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "无法确定用户目录"))?;
            if on {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let exe = std::env::current_exe()?;
                let plist = format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                     <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
                     <plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{LABEL}</string>\n  \
                     <key>ProgramArguments</key>\n  <array><string>{}</string><string>run</string><string>--no-browser</string></array>\n  \
                     <key>RunAtLoad</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n</dict>\n</plist>\n",
                    exe.display()
                );
                // 只写文件，下次登录时生效：现在 bootstrap 会立刻再启动一份。
                std::fs::write(&path, plist)?;
            } else {
                // 只删文件：bootout 会把由登录项启动的当前进程也一起结束。
                let _ = std::fs::remove_file(&path);
            }
            Ok(())
        }
    }
    #[cfg(target_os = "macos")]
    pub use imp::{is_enabled, set_enabled};

    /// macOS 第一次打开应用时默认开启登录自启（远程访问要求客户端一直在运行），之后以用户的选择为准。
    /// 旧版安装脚本的登录项（同一个 Label）指向 ~/Applications/Looklook，在这里换成应用包，
    /// 并等旧进程放开数据目录的锁。
    #[cfg(target_os = "macos")]
    pub fn default_on(home: Option<&std::path::PathBuf>) {
        let Ok(paths) = crate::paths::Paths::discover(home.cloned()) else { return };
        let marker = paths.home.join("autostart.default");
        if marker.exists() {
            return;
        }
        let _ = std::fs::write(&marker, b"");
        let plist = dirs::home_dir().map(|h| h.join("Library/LaunchAgents/com.looklook.client.plist"));
        let old = plist.and_then(|p| std::fs::read_to_string(p).ok());
        let exe = std::env::current_exe().map(|e| e.display().to_string()).unwrap_or_default();
        if old.as_deref().is_some_and(|o| o.contains(&exe)) {
            return;
        }
        if old.is_some() {
            // 旧版安装脚本的登录项还在运行旧程序：结束它（不是当前进程，路径不同）。
            let uid = unsafe { libc::getuid() };
            let _ = std::process::Command::new("launchctl").arg("bootout").arg(format!("gui/{uid}/com.looklook.client")).status();
        }
        if let Err(e) = set_enabled(true) {
            tracing::warn!(error = %e, "无法开启登录自启");
        }
        let lock = paths.home.join("run").join("lock");
        for _ in 0..30 {
            let free = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(&lock).is_ok_and(|f| f.try_lock().is_ok());
            if free {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}

/// 主窗口的大小、位置、是否最大化，存在数据目录的 `window.json`。
/// 以前每次都按 1280×820 居中新建，打开、升级后重启都回到默认大小。
mod geometry {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use tauri::{AppHandle, Manager, Runtime, Window};

    use super::Shell;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Geometry {
        /// 逻辑像素（和 `inner_size` 一样，换了缩放比例的屏幕也对）
        pub width: f64,
        pub height: f64,
        /// 外框左上角，逻辑像素；不在任何屏幕上时不用（屏幕拔掉了），改为居中
        pub position: Option<(f64, f64)>,
        pub maximized: bool,
    }

    fn file<R: Runtime>(app: &AppHandle<R>) -> Option<std::path::PathBuf> {
        let home = app.try_state::<Shell>()?.home.clone();
        Some(crate::paths::Paths::discover(home).ok()?.home.join("window.json"))
    }

    /// 读上次存的；尺寸不合理、位置不在现在任何一块屏幕上时去掉对应部分
    pub fn load<R: Runtime>(app: &AppHandle<R>) -> Option<Geometry> {
        let mut g: Geometry = serde_json::from_slice(&std::fs::read(file(app)?).ok()?).ok()?;
        if !(g.width >= 420.0 && g.height >= 480.0 && g.width < 20000.0 && g.height < 20000.0) {
            return None;
        }
        if let Some((x, y)) = g.position {
            let monitors = app.available_monitors().unwrap_or_default();
            // 标题栏那一条（左上角往里 80×40）要落在某块屏幕里，不然窗口拖不回来
            let on_screen = monitors.iter().any(|m| {
                let s = m.scale_factor();
                let (p, z) = (m.position().to_logical::<f64>(s), m.size().to_logical::<f64>(s));
                x + 80.0 > p.x && x + 80.0 < p.x + z.width && y + 10.0 >= p.y && y + 40.0 < p.y + z.height
            });
            if !on_screen {
                g.position = None;
            }
        }
        Some(g)
    }

    /// 现在的样子；最小化、全屏时不算（那时的尺寸不是用户要的）。最大化时只记“最大化”，尺寸留上次正常时的
    fn current<R: Runtime>(w: &Window<R>) -> Option<Geometry> {
        if w.is_minimized().unwrap_or(false) || w.is_fullscreen().unwrap_or(false) || !w.is_visible().unwrap_or(false) {
            return None;
        }
        let app = w.app_handle();
        let old = file(app).and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice::<Geometry>(&b).ok());
        if w.is_maximized().unwrap_or(false) {
            let mut g = old.unwrap_or(Geometry { width: 1280.0, height: 820.0, position: None, maximized: true });
            g.maximized = true;
            return Some(g);
        }
        let s = w.scale_factor().ok()?;
        let size = w.inner_size().ok()?.to_logical::<f64>(s);
        let pos = w.outer_position().ok().map(|p| p.to_logical::<f64>(s));
        Some(Geometry { width: size.width.round(), height: size.height.round(), position: pos.map(|p| (p.x.round(), p.y.round())), maximized: false })
    }

    pub fn save_now<R: Runtime>(w: &Window<R>) {
        let Some(g) = current(w) else { return };
        let Some(f) = file(w.app_handle()) else { return };
        if let Ok(b) = serde_json::to_vec(&g) {
            let _ = std::fs::write(f, b);
        }
    }

    /// 拖动、缩放时一秒内只在停下来后存一次
    pub fn save_later<R: Runtime>(w: &Window<R>) {
        static GEN: AtomicU64 = AtomicU64::new(0);
        let my = GEN.fetch_add(1, Ordering::Relaxed) + 1;
        let w = w.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(800)).await;
            if GEN.load(Ordering::Relaxed) == my {
                let h = w.clone();
                let _ = w.run_on_main_thread(move || save_now(&h));
            }
        });
    }
}
