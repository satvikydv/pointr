use crate::CaptureState;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;
use tauri::State;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

/// Types text into whatever had OS input focus at the moment of the original
/// hotkey press — no coordinates, no click, so none of the "wrong pixel" risk
/// a grounded click would carry. The caller (main.js) only reaches this after
/// the user explicitly confirmed the action's description; this command
/// itself doesn't gate anything, it just executes.
///
/// By execution time Pointr's own window has OS focus (it had to, to show
/// the ask box and the confirm prompt), so the target window's HWND —
/// captured back at `trigger_capture_direct`/`trigger_capture`, before our
/// overlay ever took focus — is restored first. This only restores
/// window-level focus, not necessarily the exact control that had it (e.g.
/// a browser-hosted compose box); most apps remember their last-focused
/// control when the window regains foreground focus, but it isn't guaranteed
/// for every app.
#[tauri::command]
pub fn execute_type_text(text: String, state: State<'_, Mutex<CaptureState>>) -> Result<(), String> {
    let target_hwnd = state.lock().unwrap().target_hwnd;
    if target_hwnd != 0 {
        unsafe {
            let _ = SetForegroundWindow(HWND(target_hwnd as *mut _));
        }
        sleep(Duration::from_millis(150)); // let the target window actually regain focus first
    }

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;
    enigo.text(&text).map_err(|e| format!("Failed to type text: {}", e))
}

/// Opens an app via the Start menu search — Win key, type the name, Enter.
/// Deliberately keystroke-only, not "find and click the result icon": no
/// coordinate grounding needed, so no risk of clicking the wrong thing.
#[tauri::command]
pub fn execute_open_app(app_name: String) -> Result<(), String> {
    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;

    enigo.key(Key::Meta, Direction::Click).map_err(|e| format!("Failed to press Win key: {}", e))?;
    sleep(Duration::from_millis(400)); // let the Start menu open and its search box take focus

    enigo.text(&app_name).map_err(|e| format!("Failed to type app name: {}", e))?;
    sleep(Duration::from_millis(350)); // let search results populate before Enter

    enigo.key(Key::Return, Direction::Click).map_err(|e| format!("Failed to press Enter: {}", e))
}
