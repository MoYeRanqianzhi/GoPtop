//! Windows Snap Layouts 透明覆盖层 —— tauri-native-chrome 技能的真实路径。
//!
//! 背景：wry 的 WebView2 子窗口宿主覆盖宿主窗口整个客户区，DWM caption 命中区
//! （HTMAXBUTTON）被遮蔽；合成宿主（ICoreWebView2CompositionController）在
//! wry 0.55 未上游化（源码核查 2026-09-13），fork 不取。采用 zbrooklyn/
//! tauri-snap-layouts 实证路线（Win11 26200 + Tauri 2.11.5 验证）：在 HTML
//! 最大化按钮正上方放一个透明原生子窗口，其 WndProc 对 WM_NCHITTEST 无条件
//! 返回 HTMAXBUTTON——OS 随之提供真实 Snap Layouts 悬停弹层。
//! **动作必须回传页面**（emit `titlebar://max-click`，前端执行 toggleMaximize）：
//! decorations:false 下根窗口没有 caption 语义，向根窗口转发 WM_NCLBUTTONDOWN
//! 是无效动作——第一版的踩坑点（技能/模板原文「必须把悬停和点击事件回传页面」）。
//! 恒返回非客户区命中码的连带后果：本窗口的鼠标输入全部走非客户区通道，只收
//! `WM_NCMOUSEMOVE`/`WM_NCMOUSELEAVE`/`WM_NCLBUTTONDOWN`，客户区消息一条不来。
//! 最小化/关闭不经本层（前端经 Tauri window API 转发，技能允许的窄例外）。
//!
//! 窗口样式约束（缺一即失效）：必须 WS_CHILD|WS_VISIBLE|WS_CLIPSIBLINGS；
//! 禁 WS_EX_LAYERED（丢命中测试）与 WS_EX_TRANSPARENT（命中穿透）。

#[cfg(windows)]
mod imp {
    use std::sync::{Mutex, OnceLock};
    use std::sync::atomic::{AtomicBool, Ordering};
    use tauri::{Emitter, WebviewWindow};
    use windows::{
        core::{w, PCWSTR},
        Win32::{
            Foundation::*,
            System::LibraryLoader::GetModuleHandleW,
            UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT},
            UI::WindowsAndMessaging::*,
        },
    };

    const CLASS_NAME: PCWSTR = w!("goptop_snap_overlay");
    const MAX_CLICK_EVENT: &str = "titlebar://max-click";
    const MAX_HOVER_EVENT: &str = "titlebar://max-hover";
    static OVERLAY_HWND: Mutex<isize> = Mutex::new(0);
    static MAX_HOVER: AtomicBool = AtomicBool::new(false);
    static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

    /// setup 阶段注入 AppHandle：覆盖层 WndProc 需要 emit 事件回传前端。
    pub fn init(app: tauri::AppHandle) {
        let _ = APP.set(app);
    }

    unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // edition 2024：unsafe fn 体内不再隐式 unsafe，逐分支显式包裹
        unsafe {
            match msg {
                // 无条件 HTMAXBUTTON：OS 据此悬停弹出 Snap Layouts
                WM_NCHITTEST => LRESULT(HTMAXBUTTON as isize),
                // 点击回传前端执行 toggleMaximize（zbrooklyn 路线的关键：decorations:false
                // 下根窗口没有 caption 语义，转发 WM_NCLBUTTONDOWN 是无效动作——
                // 第一版踩坑点；覆盖层只负责让 OS 提供 Snap 悬停弹层，动作必须回页面）
                WM_NCLBUTTONDOWN => {
                    if let Some(app) = APP.get() {
                        let _ = app.emit(MAX_CLICK_EVENT, ());
                    }
                    LRESULT(0)
                }
                // 悬停态回传：覆盖层吞掉了按钮的鼠标消息，CSS :hover 失效——
                // 状态转换（进入/离开）各发一次事件，前端手动上悬停底色（第二版踩坑点）。
                // **必须收非客户区消息**：WM_NCHITTEST 恒返回 HTMAXBUTTON（非客户区命中码），
                // 系统于是把本窗口的鼠标输入整体走非客户区通道——到达的是 WM_NCMOUSEMOVE /
                // WM_NCMOUSELEAVE，WM_MOUSEMOVE / WM_MOUSELEAVE 永不出现（第三版踩坑点：
                // 悬停时实测只收到 msg=160，客户区分支是死代码，悬停底色从不生效）。
                WM_NCMOUSEMOVE => {
                    if !MAX_HOVER.swap(true, Ordering::SeqCst) {
                        if let Some(app) = APP.get() {
                            let _ = app.emit(MAX_HOVER_EVENT, true);
                        }
                    }
                    // TME_NONCLIENT 是另一半：不加它 TrackMouseEvent 只跟踪客户区，
                    // 非客户区命中下永远不会产生离开消息，悬停底色会一直挂着不撤。
                    let tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE | TME_NONCLIENT,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&tme as *const TRACKMOUSEEVENT as *mut TRACKMOUSEEVENT);
                    LRESULT(0)
                }
                WM_NCMOUSELEAVE => {
                    MAX_HOVER.store(false, Ordering::SeqCst);
                    if let Some(app) = APP.get() {
                        let _ = app.emit(MAX_HOVER_EVENT, false);
                    }
                    LRESULT(0)
                }
                // 窗口失活时主动清悬停态：Alt+Tab 切走光标并不离开按钮，不产生
                // WM_NCMOUSELEAVE，残留的高亮会让失焦窗口一直亮着按钮（真 Windows
                // 失焦即清）。默认处理照走，别吞掉 WM_ACTIVATE。
                WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                    if MAX_HOVER.swap(false, Ordering::SeqCst) {
                        if let Some(app) = APP.get() {
                            let _ = app.emit(MAX_HOVER_EVENT, false);
                        }
                    }
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }

    /// 创建或移动覆盖层。x/y/w/h 为逻辑坐标（网页 CSS px，相对窗口客户区），
    /// 按窗口 DPI 换算物理 px；w/h 非正时隐藏。
    pub fn set_rect(window: &WebviewWindow, x: f64, y: f64, w: f64, h: f64) {
        unsafe {
            let Ok(hwnd) = window.hwnd() else { return };
            let parent = HWND(hwnd.0);
            let scale = window.scale_factor().unwrap_or(1.0);
            let (px, py) = ((x * scale).round() as i32, (y * scale).round() as i32);
            let (pw, ph) = ((w * scale).round() as i32, (h * scale).round() as i32);

            let mut cur = OVERLAY_HWND.lock().unwrap();
            let overlay = if *cur == 0 {
                let Ok(hinstance) = GetModuleHandleW(None) else { return };
                let cls = WNDCLASSW {
                    lpfnWndProc: Some(overlay_proc),
                    hInstance: hinstance.into(),
                    lpszClassName: CLASS_NAME,
                    ..Default::default()
                };
                RegisterClassW(&cls);
                let created = CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    CLASS_NAME,
                    w!(""),
                    WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS,
                    0,
                    0,
                    0,
                    0,
                    Some(parent),
                    None,
                    Some(hinstance.into()),
                    None,
                );
                match created {
                    Ok(h) => {
                        *cur = h.0 as isize;
                        h
                    }
                    Err(_) => return,
                }
            } else {
                HWND(*cur as *mut _)
            };

            if pw <= 0 || ph <= 0 {
                let _ = ShowWindow(overlay, SW_HIDE);
                return;
            }
            let _ = SetWindowPos(overlay, Some(HWND_TOP), px, py, pw, ph, SWP_NOACTIVATE);
            let _ = ShowWindow(overlay, SW_SHOWNA);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use tauri::WebviewWindow;
    pub fn init(_app: tauri::AppHandle) {}
    pub fn set_rect(_window: &WebviewWindow, _x: f64, _y: f64, _w: f64, _h: f64) {}
}

/// 前端在最大化按钮几何变化（窗口 resize/DPI/布局）时上报其逻辑矩形；
/// 仅 Windows 下创建/移动覆盖层，其余平台 no-op（无 Snap Layouts 概念）。
#[tauri::command]
pub fn snap_overlay_set_rect(window: tauri::WebviewWindow, x: f64, y: f64, w: f64, h: f64) {
    imp::set_rect(&window, x, y, w, h);
}

/// setup 阶段调用：注入 AppHandle 供覆盖层回传点击事件。
pub fn init(app: tauri::AppHandle) {
    imp::init(app);
}
