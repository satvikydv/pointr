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
//! What this can't hide: the mouse cursor, focus changes, the tray icon
//! (drawn by Explorer), or a camera pointed at the screen. Any process can
//! also ask a window for its display affinity, so this is "not visible in
//! the share", not "undetectable".

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewWindow};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowDisplayAffinity, GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, GWL_EXSTYLE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WDA_MONITOR, WDA_NONE, WINDOW_DISPLAY_AFFINITY, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

/// Every window the app creates. Windows that get created later must call
/// `protect` themselves.
const WINDOWS: [&str; 5] = ["splash", "main", "settings", "history", "meeting"];

/// Windows that show in the taskbar normally (per tauri.conf.json) and are
/// pulled out of it only while stealth is on.
const TASKBAR_WINDOWS: [&str; 2] = ["settings", "history"];

/// Always protected regardless of the setting: it only exists for meetings.
pub const MEETING_WINDOW: &str = "meeting";

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

/// Sets or clears capture exclusion on one window and reports what actually
/// took effect.
pub fn protect(window: &WebviewWindow, enabled: bool) -> Protection {
    let Some(hwnd) = hwnd_of(window) else { return Protection::Failed };
    if !enabled {
        return if set_affinity(hwnd, WDA_NONE) { Protection::Off } else { Protection::Failed };
    }
    if set_affinity(hwnd, WDA_EXCLUDEFROMCAPTURE) {
        Protection::Hidden
    } else if set_affinity(hwnd, WDA_MONITOR) {
        Protection::BlackBox
    } else {
        Protection::Failed
    }
}

/// Adds or removes extended window styles. Tauri rewrites the whole
/// extended style when some of its own flags change (click-through, for
/// one), so call this after those, not before.
fn set_ex_style(window: &WebviewWindow, bits: u32, on: bool) {
    let Some(hwnd) = hwnd_of(window) else { return };
    unsafe {
        let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let next = if on { current | bits as isize } else { current & !(bits as isize) };
        if next != current {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

/// Makes a window a non-activating tool window: never listed in Alt-Tab,
/// and clicking or showing it never takes focus from the app the user is in
/// (a meeting tab, for instance, which may notice losing focus).
pub fn make_non_activating(window: &WebviewWindow) {
    set_ex_style(window, WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0, true);
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

/// Applies the stealth setting to every window. Returns the main overlay's
/// result as the one to report: every window runs on the same Windows
/// build, so they all get the same outcome.
pub fn apply_all(app: &AppHandle, enabled: bool) -> Protection {
    let mut reported = Protection::Failed;
    for label in WINDOWS {
        let Some(window) = app.get_webview_window(label) else { continue };
        if label == MEETING_WINDOW {
            continue; // always protected, see setup_meeting_window
        }
        let result = protect(&window, enabled);
        if label == "main" {
            reported = result;
        }
        if TASKBAR_WINDOWS.contains(&label) {
            let _ = window.set_skip_taskbar(enabled);
        }
        set_ex_style(&window, WS_EX_TOOLWINDOW.0, enabled);
    }
    if cfg!(debug_assertions) {
        eprintln!("[stealth] enabled={} result={:?}", enabled, reported);
    }
    reported
}

#[derive(Serialize)]
pub struct StealthStatus {
    enabled: bool,
    protection: Protection,
}

#[tauri::command]
pub fn get_stealth_status(app: AppHandle) -> Result<StealthStatus, String> {
    let enabled = crate::commands::settings::stealth_enabled(&app);
    let protection = app
        .get_webview_window("main")
        .and_then(|w| hwnd_of(&w))
        .and_then(read_affinity)
        .map(|a| match a {
            a if a == WDA_EXCLUDEFROMCAPTURE.0 => Protection::Hidden,
            a if a == WDA_MONITOR.0 => Protection::BlackBox,
            _ => Protection::Off,
        })
        .unwrap_or(Protection::Failed);
    Ok(StealthStatus { enabled, protection })
}

#[tauri::command]
pub fn set_stealth_enabled(app: AppHandle, enabled: bool) -> Result<StealthStatus, String> {
    crate::commands::settings::persist_stealth_enabled(&app, enabled)?;
    let protection = apply_all(&app, enabled);
    Ok(StealthStatus { enabled, protection })
}
