use crate::CaptureState;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;
use tauri::State;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

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

    // Captured before anything else — the baseline to detect a real change
    // away from, whether that's Pointr's own window or whatever else was
    // focused when this command started.
    let hwnd_before = unsafe { GetForegroundWindow() };

    enigo.key(Key::Meta, Direction::Click).map_err(|e| format!("Failed to press Win key: {}", e))?;
    sleep(Duration::from_millis(400)); // let the Start menu open and its search box take focus

    enigo.text(&app_name).map_err(|e| format!("Failed to type app name: {}", e))?;
    sleep(Duration::from_millis(600)); // let search results populate before Enter — 350ms was too tight, saw stale/empty results in the multi-step loop

    enigo.key(Key::Return, Direction::Click).map_err(|e| format!("Failed to press Enter: {}", e))?;

    // Round 3 on this same problem — rounds 1 (foreground re-assert alone)
    // and 2 (+ a click at a single fixed-delay-later read of
    // GetForegroundWindow) both turned out to still fail on a live run,
    // audibly: Windows played its own "can't route this keystroke" beep,
    // which happens when input focus is on the bare desktop rather than a
    // real control. That points at the actual root cause — a single read
    // of GetForegroundWindow() 700ms after Enter can still be the Search UI
    // or the desktop, not the launched app, if that app is cold-starting
    // slowly (first launch in a session especially). Poll instead of
    // trusting one fixed delay: wait for the foreground window to actually
    // change away from hwnd_before AND hold steady for one more poll
    // (not just caught mid-transition), up to 3s.
    let fg: HWND;
    let mut prev = hwnd_before;
    let mut waited_ms = 0u32;
    loop {
        sleep(Duration::from_millis(150));
        waited_ms += 150;
        let current = unsafe { GetForegroundWindow() };
        if current != hwnd_before && !current.is_invalid() && current == prev {
            fg = current;
            break;
        }
        prev = current;
        if waited_ms >= 3000 {
            fg = current; // best available guess if it never settled in time
            break;
        }
    }
    sleep(Duration::from_millis(250)); // let it finish laying out before anything looks at it

    // Window-level foreground only — no blind click into a guessed window
    // center here. Accelerators (Ctrl+S) dispatch at the window level, so
    // this alone is enough for those; the model itself is given an explicit
    // instruction (agent.py's step prompt) to click into the actual content
    // area as its own visible next step before typing, since it can see the
    // screenshot and land somewhere real, instead of Rust guessing blind.
    unsafe {
        if !fg.is_invalid() {
            let _ = enigo.key(Key::Alt, Direction::Press);
            let _ = enigo.key(Key::Alt, Direction::Release);
            let _ = SetForegroundWindow(fg);
        }
    }

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
/// `button` ("left"/"right", default left) and `double` (default false) —
/// the multi-step loop's click action gained these so right-click context
/// menus and double-click-to-open are reachable without a separate action
/// type; the coordinate math itself (relative-move workaround for enigo's
/// broken multi-monitor Abs mode) is unchanged.
#[tauri::command]
pub fn execute_click(
    x_norm: f32,
    y_norm: f32,
    button: Option<String>,
    double: Option<bool>,
    state: State<'_, Mutex<CaptureState>>,
) -> Result<(), String> {
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

    let btn = match button.as_deref() {
        Some("right") => Button::Right,
        _ => Button::Left,
    };

    enigo.button(btn, Direction::Click).map_err(|e| format!("Failed to click: {}", e))?;
    if double.unwrap_or(false) {
        sleep(Duration::from_millis(60)); // real double-clicks aren't instant either — too fast and some apps read it as two singles
        enigo.button(btn, Direction::Click).map_err(|e| format!("Failed to click: {}", e))?;
    }
    Ok(())
}

/// Scrolls the mouse wheel. `direction` is "up"/"down"/"left"/"right",
/// `amount` is wheel notches (default 3 — enough to move a normal list/page
/// without overshooting). Confirmed against enigo's own Windows backend
/// (Button::ScrollDown maps to scroll(1, Vertical)) rather than assumed:
/// positive length scrolls down/right, negative scrolls up/left.
#[tauri::command]
pub fn execute_scroll(direction: String, amount: Option<i32>) -> Result<(), String> {
    let notches = amount.unwrap_or(3).abs().max(1);
    let (length, axis) = match direction.as_str() {
        "down" => (notches, Axis::Vertical),
        "up" => (-notches, Axis::Vertical),
        "right" => (notches, Axis::Horizontal),
        "left" => (-notches, Axis::Horizontal),
        other => return Err(format!("Unsupported scroll direction: {}", other)),
    };

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;
    enigo.scroll(length, axis).map_err(|e| format!("Failed to scroll: {}", e))
}

// Dedicated VK-mapped variants for A-Z / 0-9 — hit a real failure using
// Key::Unicode for this instead: "Ctrl+S" threw "could not translate the
// character to the corresponding virtual-key code and shift state for the
// current keyboard". Key::Unicode goes through a char->keysym->VK
// translation path that can fail outright on Windows; Key::A..Key::Z and
// Key::Num0..Key::Num9 map straight to real VK codes (confirmed in enigo's
// own keycodes.rs) and skip that lookup entirely.
fn letter_key(c: char) -> Result<Key, String> {
    Ok(match c.to_ascii_uppercase() {
        'A' => Key::A, 'B' => Key::B, 'C' => Key::C, 'D' => Key::D, 'E' => Key::E,
        'F' => Key::F, 'G' => Key::G, 'H' => Key::H, 'I' => Key::I, 'J' => Key::J,
        'K' => Key::K, 'L' => Key::L, 'M' => Key::M, 'N' => Key::N, 'O' => Key::O,
        'P' => Key::P, 'Q' => Key::Q, 'R' => Key::R, 'S' => Key::S, 'T' => Key::T,
        'U' => Key::U, 'V' => Key::V, 'W' => Key::W, 'X' => Key::X, 'Y' => Key::Y,
        'Z' => Key::Z,
        other => return Err(format!("Unsupported letter key: {}", other)),
    })
}

fn digit_key(c: char) -> Result<Key, String> {
    Ok(match c {
        '0' => Key::Num0, '1' => Key::Num1, '2' => Key::Num2, '3' => Key::Num3, '4' => Key::Num4,
        '5' => Key::Num5, '6' => Key::Num6, '7' => Key::Num7, '8' => Key::Num8, '9' => Key::Num9,
        other => return Err(format!("Unsupported digit key: {}", other)),
    })
}

fn parse_named_key(name: &str) -> Result<Key, String> {
    Ok(match name {
        "Enter" => Key::Return,
        "Tab" => Key::Tab,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "ArrowDown" => Key::DownArrow,
        "ArrowUp" => Key::UpArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        _ if name.len() == 1 && name.chars().next().unwrap().is_ascii_alphabetic() => {
            letter_key(name.chars().next().unwrap())?
        }
        _ if name.len() == 1 && name.chars().next().unwrap().is_ascii_digit() => {
            digit_key(name.chars().next().unwrap())?
        }
        // Fallback for anything else single-char (punctuation) — still goes
        // through the translation path above, but that's not the case that
        // actually broke (plain letters/digits, the vast majority of real
        // shortcuts, no longer touch it at all).
        other if other.chars().count() == 1 => Key::Unicode(other.chars().next().unwrap()),
        other => return Err(format!("Unsupported key: {}", other)),
    })
}

fn parse_modifier_key(name: &str) -> Result<Key, String> {
    Ok(match name {
        "Ctrl" | "Control" => Key::Control,
        "Alt" => Key::Alt,
        "Shift" => Key::Shift,
        "Win" | "Meta" | "Cmd" => Key::Meta,
        other => return Err(format!("Unsupported modifier: {}", other)),
    })
}

/// Parses "Enter" or "Ctrl+Shift+S" into (main key, modifier keys in press
/// order) — split out from execute_key_press so the parsing itself (the
/// part actually easy to get subtly wrong: which segment is the real key,
/// empty segments from stray "+"s) is unit-testable without touching enigo.
fn parse_key_combo(key: &str) -> Result<(Key, Vec<Key>), String> {
    let parts: Vec<&str> = key.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
    let Some((&main_name, modifier_names)) = parts.split_last() else {
        return Err(format!("Unsupported key: {}", key));
    };

    let main_key = parse_named_key(main_name)?;
    let modifiers = modifier_names
        .iter()
        .map(|m| parse_modifier_key(m))
        .collect::<Result<Vec<Key>, String>>()?;
    Ok((main_key, modifiers))
}

/// A key press, optionally with modifiers — "Enter" for a bare key, or
/// "Ctrl+S" / "Alt+Tab" / "Ctrl+Shift+Escape" for a combo (modifiers
/// separated by "+", the real key last). Modifiers are held down for the
/// main key's click and released after, in reverse press order.
#[tauri::command]
pub fn execute_key_press(key: String) -> Result<(), String> {
    let (main_key, modifiers) = parse_key_combo(&key)?;

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;

    for m in &modifiers {
        enigo.key(*m, Direction::Press).map_err(|e| format!("Failed to press modifier for {}: {}", key, e))?;
    }
    let result = enigo.key(main_key, Direction::Click).map_err(|e| format!("Failed to press {}: {}", key, e));
    for m in modifiers.iter().rev() {
        // Release modifiers even if the main key click failed above — an
        // unreleased Ctrl left held down would silently mangle every
        // subsequent keystroke for the rest of the session.
        let _ = enigo.key(*m, Direction::Release);
    }
    result
}

#[cfg(test)]
mod key_combo_tests {
    use super::*;

    #[test]
    fn bare_named_key_has_no_modifiers() {
        let (main, mods) = parse_key_combo("Enter").expect("should parse");
        assert_eq!(main, Key::Return);
        assert!(mods.is_empty());
    }

    #[test]
    fn bare_letter_maps_to_dedicated_key_variant() {
        // Not Key::Unicode — that's the path that actually broke ("Ctrl+S"
        // errored with a real VK-translation failure on a real run).
        let (main, mods) = parse_key_combo("s").expect("should parse");
        assert_eq!(main, Key::S);
        assert!(mods.is_empty());
    }

    #[test]
    fn single_modifier_combo_orders_modifier_before_key() {
        let (main, mods) = parse_key_combo("Ctrl+S").expect("should parse");
        assert_eq!(main, Key::S);
        assert_eq!(mods, vec![Key::Control]);
    }

    #[test]
    fn digit_key_maps_to_dedicated_variant() {
        let (main, mods) = parse_key_combo("Ctrl+1").expect("should parse");
        assert_eq!(main, Key::Num1);
        assert_eq!(mods, vec![Key::Control]);
    }

    #[test]
    fn multi_modifier_combo_preserves_press_order() {
        let (main, mods) = parse_key_combo("Ctrl+Shift+Escape").expect("should parse");
        assert_eq!(main, Key::Escape);
        assert_eq!(mods, vec![Key::Control, Key::Shift]);
    }

    #[test]
    fn alt_tab_parses() {
        let (main, mods) = parse_key_combo("Alt+Tab").expect("should parse");
        assert_eq!(main, Key::Tab);
        assert_eq!(mods, vec![Key::Alt]);
    }

    #[test]
    fn unknown_modifier_is_rejected() {
        assert!(parse_key_combo("Fn+S").is_err());
    }

    #[test]
    fn unknown_named_key_is_rejected() {
        assert!(parse_key_combo("PageUp").is_err());
    }

    #[test]
    fn empty_string_is_rejected() {
        assert!(parse_key_combo("").is_err());
    }

    #[test]
    fn stray_plus_signs_are_ignored_as_empty_segments() {
        // "Ctrl++S" (a double "+", e.g. from a trimming bug upstream)
        // shouldn't parse as a modifier literally named "".
        let (main, mods) = parse_key_combo("Ctrl++S").expect("should parse");
        assert_eq!(main, Key::S);
        assert_eq!(mods, vec![Key::Control]);
    }
}
