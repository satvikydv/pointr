use crate::CaptureState;
use enigo::{Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;
use tauri::State;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;

/// Types text into whatever currently has OS input focus — no coordinates,
/// no click, so none of the "wrong pixel" risk a grounded click would carry.
/// The caller (main.js) only reaches this after the user explicitly
/// confirmed the action's description; this command itself doesn't gate
/// anything, it just executes.
///
/// `restore_original_focus` (default true when omitted) controls whether
/// the window that had focus at the *original hotkey press* — captured back
/// in `trigger_capture_direct`/`trigger_capture`, before our overlay ever
/// took it — gets refocused first. That's correct for the single-action
/// confirm flow (type into the chat box the user was looking at when they
/// pressed the hotkey), but wrong for the multi-step loop: after a step
/// opens Notepad, typing needs to go into Notepad, not get yanked back to
/// whatever was open before the hotkey. The multi-step loop passes `false`
/// so this only restores window-level focus, not necessarily the exact
/// control that had it (e.g. a browser-hosted compose box); most apps
/// remember their last-focused control when the window regains foreground
/// focus, but it isn't guaranteed for every app.
#[tauri::command]
pub fn execute_type_text(
    text: String,
    restore_original_focus: Option<bool>,
    state: State<'_, Mutex<CaptureState>>,
) -> Result<(), String> {
    if restore_original_focus.unwrap_or(true) {
        let target_hwnd = state.lock().unwrap().target_hwnd;
        if target_hwnd != 0 {
            unsafe {
                let _ = SetForegroundWindow(HWND(target_hwnd as *mut _));
            }
            sleep(Duration::from_millis(150)); // let the target window actually regain focus first
        }
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
    sleep(Duration::from_millis(600)); // let search results populate before Enter — 350ms was too tight, saw stale/empty results in the multi-step loop

    enigo.key(Key::Return, Direction::Click).map_err(|e| format!("Failed to press Enter: {}", e))?;
    sleep(Duration::from_millis(700)); // give the launched app a moment to actually start painting before anything (e.g. a follow-up screenshot) looks at the screen
    Ok(())
}

/// Clicks at a normalized position within the monitor the current multi-step
/// automation is running on. `x_norm`/`y_norm` are 0.0-1.0, same convention
/// as `pointer_target`/storyboard coordinates elsewhere in the app (the
/// JS layer is what converts Gemini's native `[y, x]` 0-1000 point format
/// into these before calling this command).
///
/// Deliberately moves the cursor via a RELATIVE offset from its current
/// position, not `enigo`'s `Coordinate::Abs` — enigo 0.2's Windows Abs mode
/// normalizes against `GetSystemMetrics(SM_CXSCREEN)`, which is only ever
/// the PRIMARY monitor's dimensions, so an absolute move targeting a
/// secondary monitor lands in the wrong place entirely (confirmed by
/// reading enigo's own Windows backend source, not assumed). Relative
/// movement — current position (real `GetCursorPos`, virtual-desktop-space,
/// matching this app's own `capture/cursor.rs`) plus a computed delta —
/// sidesteps that bug on any monitor arrangement.
#[tauri::command]
pub fn execute_click(x_norm: f32, y_norm: f32, state: State<'_, Mutex<CaptureState>>) -> Result<(), String> {
    let monitor = state
        .lock()
        .unwrap()
        .monitor
        .clone()
        .ok_or_else(|| "No monitor recorded for this session".to_string())?;

    let target_x = monitor.origin_x + (x_norm.clamp(0.0, 1.0) * monitor.width_px as f32) as i32;
    let target_y = monitor.origin_y + (y_norm.clamp(0.0, 1.0) * monitor.height_px as f32) as i32;

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;
    let (current_x, current_y) = enigo.location().map_err(|e| format!("Failed to read cursor position: {}", e))?;

    enigo
        .move_mouse(target_x - current_x, target_y - current_y, Coordinate::Rel)
        .map_err(|e| format!("Failed to move mouse: {}", e))?;
    sleep(Duration::from_millis(60)); // let the OS/app register the pointer at the new position before clicking

    enigo
        .button(enigo::Button::Left, Direction::Click)
        .map_err(|e| format!("Failed to click: {}", e))
}

/// One named key press — the small fixed vocabulary the multi-step loop's
/// prompt offers (Enter/Tab/Escape/Backspace/arrows), not arbitrary key
/// names, so there's no free-text key-name parsing to get wrong.
#[tauri::command]
pub fn execute_key_press(key: String) -> Result<(), String> {
    let mapped = match key.as_str() {
        "Enter" => Key::Return,
        "Tab" => Key::Tab,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "ArrowDown" => Key::DownArrow,
        "ArrowUp" => Key::UpArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        other => return Err(format!("Unsupported key: {}", other)),
    };

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;
    enigo.key(mapped, Direction::Click).map_err(|e| format!("Failed to press {}: {}", key, e))
}
