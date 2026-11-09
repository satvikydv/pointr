//! Stealth: keep Pointr's windows out of screen shares, recordings and
//! screenshots while they stay visible on the user's own monitor.
//!
//! The mechanism is `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`
//! (Windows 10 2004 / build 19041+). The compositor removes the window from
//! every frame handed to capture APIs (Windows Graphics Capture, Desktop
//! Duplication, GDI BitBlt of the desktop), which is what Zoom, Teams, Meet
//! in a browser, OBS and the Snipping Tool all use.
//!
//! Tauri wraps the same call (`set_content_protected`) but discards the
//! result. On Windows 10 1903/1909, which Pointr still supports, the call
//! fails and the window would stay fully visible in a share without anyone
//! knowing. So the call is made here and read back with
//! `GetWindowDisplayAffinity`. If exclusion isn't available, `WDA_MONITOR`
//! is the fallback: the window shows as a black rectangle in the share.
//! That isn't hidden, but its content doesn't leak.
//!
//! Hidden windows are never touched. Clearing the affinity of a window that
//! has never been shown (settings, history and the overlay all start
//! hidden) makes DWM draw it as a blank white rectangle at its default
//! position, and it stays there. So a window's affinity is set while it is
//! visible, or at the moment it is about to be shown: every window carries a
//! small subclass that applies the current setting just before a show goes
//! through. Whether the Windows build supports exclusion at all is probed
//! once, on a throwaway hidden window.
//!
//! What this can't hide: the mouse cursor, focus changes, the tray icon
//! (drawn by Explorer), or a camera pointed at the screen. Any process can
//! also ask a window for its display affinity, so this is "not visible in
//! the share", not "undetectable".

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewWindow};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetWindowDisplayAffinity, IsWindowVisible, SetWindowDisplayAffinity,
    ShowWindow, SW_HIDE, SW_SHOWNOACTIVATE, SWP_SHOWWINDOW, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WDA_NONE,
    WINDOWPOS, WINDOW_DISPLAY_AFFINITY, WINDOW_EX_STYLE, WM_SHOWWINDOW, WM_WINDOWPOSCHANGING, WS_POPUP,
};

/// Every window the app creates. Windows that get created later must be
/// hooked with `watch` themselves.
const WINDOWS: [&str; 6] = ["splash", "main", "settings", "history", "meeting", "code"];

/// Windows that show in the taskbar normally (per tauri.conf.json) and are
/// pulled out of it only while stealth is on.
const TASKBAR_WINDOWS: [&str; 2] = ["settings", "history"];

/// Always protected regardless of the setting: it only exists for meetings.
pub const MEETING_WINDOW: &str = "meeting";

/// The coding-solution panel (meeting/code.rs): always protected too, for
/// the same reason.
pub const CODE_WINDOW: &str = "code";

const SUBCLASS_ID: usize = 0x504F_4E54;

/// The setting as last applied. Read by the show hook on whatever thread
/// the window lives on.
static DESIRED: AtomicBool = AtomicBool::new(false);
/// The meeting panel's HWND (0 until known): protected whatever `DESIRED` says.
static PANEL: AtomicIsize = AtomicIsize::new(0);
/// The code panel's HWND (0 until known): likewise always protected.
static CODE_PANEL: AtomicIsize = AtomicIsize::new(0);
/// What this Windows build can do, found out once.
static SUPPORT: OnceLock<Protection> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    /// Excluded from capture: invisible in shares.
    Hidden,
    /// This Windows build can't exclude windows; shown as a black box.
    BlackBox,
    /// Protection deliberately turned off.
    Off,
    /// Neither call took effect.
    Failed,
}

fn hwnd_of(window: &WebviewWindow) -> Option<HWND> {
    // Tauri may link a different `windows` crate version than ours, so go
    // through the raw pointer rather than sharing the type.
    window.hwnd().ok().map(|h| HWND(h.0 as _))
}

fn read_affinity(hwnd: HWND) -> Option<u32> {
    let mut value = 0u32;
    unsafe { GetWindowDisplayAffinity(hwnd, &mut value) }.ok().map(|_| value)
}

fn set_affinity(hwnd: HWND, affinity: WINDOW_DISPLAY_AFFINITY) -> bool {
    unsafe { SetWindowDisplayAffinity(hwnd, affinity) }.is_ok() && read_affinity(hwnd) == Some(affinity.0)
}

/// Tries exclusion, then the black-box fallback, on a hidden window that
/// exists only for this and is destroyed straight away.
fn probe() -> Protection {
    let hwnd = unsafe {
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!(""), WS_POPUP, 0, 0, 1, 1, None, None, None, None)
    };
    let Ok(hwnd) = hwnd else { return Protection::Failed };
    let result = if set_affinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
        Protection::Hidden
    } else if set_affinity(hwnd, WDA_MONITOR) {
        Protection::BlackBox
    } else {
        Protection::Failed
    };
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    result
}

fn support() -> Protection {
    *SUPPORT.get_or_init(probe)
}

/// What a window's affinity should be right now, or None if it can't be
/// protected on this Windows build.
fn wanted(hwnd: HWND) -> Option<WINDOW_DISPLAY_AFFINITY> {
    let always = hwnd.0 as isize == PANEL.load(Ordering::SeqCst) || hwnd.0 as isize == CODE_PANEL.load(Ordering::SeqCst);
    if DESIRED.load(Ordering::SeqCst) || always {
        match support() {
            Protection::Hidden => Some(WDA_EXCLUDEFROMCAPTURE),
            Protection::BlackBox => Some(WDA_MONITOR),
            _ => None,
        }
    } else {
        Some(WDA_NONE)
    }
}

/// Brings one window's affinity in line with the setting. A visible window
/// is only touched if it differs. `force` writes regardless: a window that
/// has been hidden reads back 0, but Windows puts its old affinity back when
/// it is shown again, so the read says nothing about what it will show with.
fn sync(hwnd: HWND, force: bool) {
    if let Some(want) = wanted(hwnd) {
        if force || read_affinity(hwnd) != Some(want.0) {
            if let Err(e) = unsafe { SetWindowDisplayAffinity(hwnd, want) } {
                if cfg!(debug_assertions) {
                    eprintln!("[stealth] affinity {} on {:#x} failed: {}", want.0, hwnd.0 as usize, e);
                }
            }
        }
    }
}

/// Runs just before a window is shown (WM_WINDOWPOSCHANGING carries
/// SWP_SHOWWINDOW before the window becomes visible; WM_SHOWWINDOW covers
/// the rest), so it never appears unprotected or with a stale affinity.
unsafe extern "system" fn on_message(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let showing = match msg {
        WM_WINDOWPOSCHANGING => (*(lparam.0 as *const WINDOWPOS)).flags.0 & SWP_SHOWWINDOW.0 != 0,
        WM_SHOWWINDOW => wparam.0 != 0,
        _ => false,
    };
    if showing {
        sync(hwnd, true);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Hooks a window so its affinity is applied whenever it is shown. Must run
/// on the thread that owns the window (Tauri's main thread).
pub fn watch(window: &WebviewWindow) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(on_message), SUBCLASS_ID, 0);
        }
    }
}

fn is_visible(window: &WebviewWindow) -> bool {
    hwnd_of(window).is_some_and(|h| unsafe { IsWindowVisible(h) }.as_bool())
}

/// Run once at startup, on the main thread: reads the setting, hooks every
/// window, and protects the ones already showing (the splash). Windows that
/// start hidden are left alone until they are shown.
pub fn init(app: &AppHandle) {
    DESIRED.store(crate::commands::settings::stealth_enabled(app), Ordering::SeqCst);
    if let Some(hwnd) = app.get_webview_window(MEETING_WINDOW).as_ref().and_then(hwnd_of) {
        PANEL.store(hwnd.0 as isize, Ordering::SeqCst);
    }
    if let Some(hwnd) = app.get_webview_window(CODE_WINDOW).as_ref().and_then(hwnd_of) {
        CODE_PANEL.store(hwnd.0 as isize, Ordering::SeqCst);
    }
    for label in WINDOWS {
        let Some(window) = app.get_webview_window(label) else { continue };
        watch(&window);
        if is_visible(&window) {
            if let Some(hwnd) = hwnd_of(&window) {
                sync(hwnd, false);
            }
        }
        // Out of the taskbar while stealth is on. Safe on a hidden window:
        // it only removes a button.
        if DESIRED.load(Ordering::SeqCst) && TASKBAR_WINDOWS.contains(&label) {
            let _ = window.set_skip_taskbar(true);
        }
    }
    if cfg!(debug_assertions) {
        eprintln!("[stealth] init enabled={} support={:?}", DESIRED.load(Ordering::SeqCst), support());
    }
    debug_dump_around(app, "startup");
}

/// What the meeting panel is protected by on this Windows build. The panel
/// itself is protected by `init` and the show hook.
pub fn panel_protection() -> Protection {
    support()
}

/// `WebviewWindow::show` activates the window. This shows it without
/// taking focus.
pub fn show_without_focus(window: &WebviewWindow) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// Whether a showing window is out of capture right now (excluded, or
/// black-boxed on old Windows). Only meaningful for a visible window: a
/// hidden one reads 0 whatever it will show with.
pub fn is_protected(window: &WebviewWindow) -> bool {
    hwnd_of(window)
        .and_then(read_affinity)
        .is_some_and(|a| a == WDA_EXCLUDEFROMCAPTURE.0 || a == WDA_MONITOR.0)
}

/// The counterpart of `show_without_focus`. tao hides a window only when its
/// own visible flag changes, and a raw `ShowWindow` never sets that flag, so
/// `WebviewWindow::hide` would do nothing for a window shown that way.
pub fn hide(window: &WebviewWindow) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// Applies the stealth setting. Windows that are showing change right away;
/// hidden ones pick it up when they're shown (see the module docs).
pub fn apply_all(app: &AppHandle, enabled: bool) -> Protection {
    debug_dump(app, &format!("before apply_all({})", enabled));
    DESIRED.store(enabled, Ordering::SeqCst);
    for label in WINDOWS {
        if label == MEETING_WINDOW || label == CODE_WINDOW {
            continue; // always protected, see `init`
        }
        let Some(window) = app.get_webview_window(label) else { continue };
        let visible = is_visible(&window);
        if visible {
            if let Some(hwnd) = hwnd_of(&window) {
                sync(hwnd, false);
            }
        }
        // Only put a window back on the taskbar if it's showing: adding a
        // hidden window gives it a taskbar button, and clicking that shows
        // a window that was meant to stay hidden. A hidden one gets its
        // button back by itself the next time it's shown.
        if TASKBAR_WINDOWS.contains(&label) && (enabled || visible) {
            let _ = window.set_skip_taskbar(enabled);
        }
    }
    let reported = if enabled { support() } else { Protection::Off };
    if cfg!(debug_assertions) {
        eprintln!("[stealth] enabled={} result={:?}", enabled, reported);
    }
    debug_dump_around(app, &format!("after apply_all({})", enabled));
    reported
}

/// Debug builds only: logs every top-level window this process owns (label,
/// visibility, DWM cloaking, geometry, styles, display affinity), to find
/// out which window is on screen when and what made it so.
#[cfg(debug_assertions)]
pub fn debug_dump(app: &AppHandle, tag: &str) {
    use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
        GWL_EXSTYLE, GWL_STYLE,
    };

    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<HWND>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            list.push(hwnd);
        }
        BOOL(1)
    }

    let mut hwnds: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut hwnds as *mut Vec<HWND> as isize));
    }
    eprintln!("[stealth] -- windows: {} ({} top-level)", tag, hwnds.len());
    for hwnd in hwnds {
        let label = app
            .webview_windows()
            .into_iter()
            .find(|(_, w)| hwnd_of(w) == Some(hwnd))
            .map(|(l, _)| l)
            .unwrap_or_else(|| "-".into());
        unsafe {
            let mut class = [0u16; 128];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let mut title = [0u16; 128];
            let t = GetWindowTextW(hwnd, &mut title) as usize;
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                &mut cloaked as *mut u32 as *mut _,
                std::mem::size_of::<u32>() as u32,
            );
            let mut rect = RECT::default();
            let _ = GetWindowRect(hwnd, &mut rect);
            eprintln!(
                "[stealth]   {:#x} label={} class={} title={:?} visible={} cloaked={} rect=({},{} {}x{}) style={:#x} exstyle={:#x} affinity={:?}",
                hwnd.0 as usize,
                label,
                String::from_utf16_lossy(&class[..n]),
                String::from_utf16_lossy(&title[..t]),
                IsWindowVisible(hwnd).as_bool(),
                cloaked,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                GetWindowLongPtrW(hwnd, GWL_STYLE) as u32,
                GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32,
                read_affinity(hwnd),
            );
        }
    }
}

#[cfg(not(debug_assertions))]
pub fn debug_dump(_app: &AppHandle, _tag: &str) {}

/// `debug_dump` now, and again a moment later once whatever was queued has
/// run.
pub fn debug_dump_around(app: &AppHandle, tag: &str) {
    if !cfg!(debug_assertions) {
        return;
    }
    debug_dump(app, &format!("{} (now)", tag));
    let app = app.clone();
    let tag = tag.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(1500));
        debug_dump(&app, &format!("{} (+1.5s)", tag));
    });
}

#[derive(Serialize)]
pub struct StealthStatus {
    enabled: bool,
    protection: Protection,
}

#[tauri::command]
pub fn get_stealth_status(app: AppHandle) -> Result<StealthStatus, String> {
    let enabled = crate::commands::settings::stealth_enabled(&app);
    let protection = if enabled { support() } else { Protection::Off };
    Ok(StealthStatus { enabled, protection })
}

#[tauri::command]
pub fn set_stealth_enabled(app: AppHandle, enabled: bool) -> Result<StealthStatus, String> {
    crate::commands::settings::persist_stealth_enabled(&app, enabled)?;
    if enabled {
        // Narration never plays in stealth mode (commands/tts.rs), and
        // that includes anything already mid-sentence.
        let _ = crate::commands::tts::stop_speech(app.state());
    }
    let protection = apply_all(&app, enabled);
    Ok(StealthStatus { enabled, protection })
}
