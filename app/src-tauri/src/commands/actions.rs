use crate::CaptureState;
use enigo::{Axis, Button, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;
use tauri::State;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetCursorPos, SetForegroundWindow};

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

    // Wait for the launched app to actually become the foreground window
    // (changed away from hwnd_before, and steady for one more poll so it
    // isn't caught mid-transition), up to 3s. A single read at a fixed
    // delay could still see the Search UI or the desktop when an app
    // cold-starts slowly, and typing then went nowhere.
    //
    // Deliberately NO foreground re-assert after this (no SetForegroundWindow,
    // no "harmless" Alt tap to satisfy the foreground-lock rule). The poll
    // already guarantees the new app is foreground, so a re-assert is a
    // no-op — and the Alt tap was actively harmful: it landed on the new
    // window and switched its menu bar into keyboard mode. Seen in a real
    // debug screenshot: Notepad open with File focused and F/E/V key-tips
    // showing, every typed character swallowed by menu navigation, the
    // document left empty. cmd was unaffected (no Alt menu), which is why
    // the same run typed into cmd fine and then failed in Notepad.
    let mut prev = hwnd_before;
    let mut waited_ms = 0u32;
    loop {
        sleep(Duration::from_millis(150));
        waited_ms += 150;
        let current = unsafe { GetForegroundWindow() };
        if current != hwnd_before && !current.is_invalid() && current == prev {
            break;
        }
        prev = current;
        if waited_ms >= 3000 {
            break;
        }
    }
    sleep(Duration::from_millis(250)); // let it finish laying out before anything looks at it

    Ok(())
}

/// After an input that may have popped up a new window (Ctrl+S opening
/// Save As, a click on a button that opens a dialog or menu), waits for
/// that window to settle before returning, so the multi-step loop's next
/// screenshot shows it. Seen in a real debug screenshot: Ctrl+S had
/// already moved activation to the Save As dialog (Notepad's caption
/// buttons went grey) but the dialog wasn't painted yet, so the model saw
/// no dialog, pressed Ctrl+S again, and the run stopped as stuck.
///
/// Cheap when nothing pops up: returns after ~400ms if the foreground
/// window never changes.
fn wait_for_foreground_settle(before: HWND) {
    const POLL_MS: u64 = 100;
    const NOTHING_CHANGED_MS: u64 = 400;
    const MAX_MS: u64 = 2500;

    let mut waited = 0;
    let mut changed = false;
    let mut prev = before;
    loop {
        sleep(Duration::from_millis(POLL_MS));
        waited += POLL_MS;
        let current = unsafe { GetForegroundWindow() };
        if current != before {
            changed = true;
        }
        if !changed && waited >= NOTHING_CHANGED_MS {
            return;
        }
        // Changed, and the same window two polls in a row — it's the one
        // that's staying, not a transient mid-switch reading.
        if changed && current == prev && !current.is_invalid() {
            break;
        }
        prev = current;
        if waited >= MAX_MS {
            break;
        }
    }
    // Activation happens before first paint, so a settled HWND can still
    // be an empty frame for a moment.
    sleep(Duration::from_millis(400));
}

/// Maps a 0.0-1.0 position within a monitor to an absolute virtual-desktop
/// screen coordinate. Split out of `execute_click` purely so the mapping
/// can be tested directly (see `click_coordinate_tests`) — "the model said
/// [y, x] but the mouse went somewhere else" is otherwise only diagnosable
/// by watching the physical cursor.
pub fn normalized_to_screen(x_norm: f32, y_norm: f32, monitor: &crate::capture::cursor::MonitorInfo) -> (i32, i32) {
    let x = monitor.origin_x + (x_norm.clamp(0.0, 1.0) * monitor.width_px as f32) as i32;
    let y = monitor.origin_y + (y_norm.clamp(0.0, 1.0) * monitor.height_px as f32) as i32;
    (x, y)
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

    let (target_x, target_y) = normalized_to_screen(x_norm, y_norm, &monitor);

    // SetCursorPos, not an enigo mouse move. Both of enigo's options are
    // wrong here:
    //   - Coordinate::Abs normalizes against GetSystemMetrics(SM_CXSCREEN),
    //     i.e. the PRIMARY monitor only, so it lands nowhere near the mark
    //     on a secondary display.
    //   - Coordinate::Rel emits MOUSEEVENTF_MOVE deltas, which Windows
    //     scales by the user's pointer-speed / Enhanced Pointer Precision
    //     settings. Measured on this machine: a delta meant to reach
    //     (760, 157) actually landed at (1528, 312) — almost exactly 2x —
    //     which is precisely the "coordinates look right but the mouse
    //     goes somewhere else" symptom.
    // SetCursorPos takes absolute virtual-desktop coordinates (what
    // normalized_to_screen already produces, monitor origin included) and
    // is unaffected by pointer speed or acceleration.
    unsafe {
        SetCursorPos(target_x, target_y)
            .map_err(|e| format!("Failed to move cursor to ({}, {}): {}", target_x, target_y, e))?;
    }

    let mut enigo = Enigo::new(&Settings::default()).map_err(|e| format!("Failed to init input simulation: {}", e))?;
    sleep(Duration::from_millis(60)); // let the OS/app register the pointer at the new position before clicking

    let btn = match button.as_deref() {
        Some("right") => Button::Right,
        _ => Button::Left,
    };

    let before = unsafe { GetForegroundWindow() };
    enigo.button(btn, Direction::Click).map_err(|e| format!("Failed to click: {}", e))?;
    if double.unwrap_or(false) {
        sleep(Duration::from_millis(60)); // real double-clicks aren't instant either — too fast and some apps read it as two singles
        enigo.button(btn, Direction::Click).map_err(|e| format!("Failed to click: {}", e))?;
    }
    wait_for_foreground_settle(before);
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
    let before = unsafe { GetForegroundWindow() };

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
    result?;
    wait_for_foreground_settle(before);
    Ok(())
}

#[cfg(test)]
mod click_coordinate_tests {
    use super::*;
    use crate::capture::cursor::MonitorInfo;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

    fn monitor(width_px: u32, height_px: u32, origin_x: i32, origin_y: i32) -> MonitorInfo {
        MonitorInfo { origin_x, origin_y, width_px, height_px, dpi: 96 }
    }

    /// The exact conversion the multi-step loop performs: Gemini returns
    /// [y, x] in 0-1000 space, main.js divides both by 1000, and the result
    /// lands here as x_norm/y_norm.
    fn gemini_point_to_screen(point_y: f32, point_x: f32, m: &MonitorInfo) -> (i32, i32) {
        normalized_to_screen(point_x / 1000.0, point_y / 1000.0, m)
    }

    #[test]
    fn maps_normalized_position_to_monitor_pixels() {
        let m = monitor(1536, 864, 0, 0);
        assert_eq!(normalized_to_screen(0.0, 0.0, &m), (0, 0));
        assert_eq!(normalized_to_screen(0.5, 0.5, &m), (768, 432));
        assert_eq!(normalized_to_screen(1.0, 1.0, &m), (1536, 864));
    }

    #[test]
    fn respects_monitor_origin_for_a_secondary_display() {
        // A monitor placed to the right of a 1920-wide primary: the same
        // normalized centre must land 1920px further right, not at 768.
        let m = monitor(1536, 864, 1920, 0);
        assert_eq!(normalized_to_screen(0.5, 0.5, &m), (1920 + 768, 432));
    }

    #[test]
    fn clamps_out_of_range_values_instead_of_flying_off_screen() {
        let m = monitor(1536, 864, 0, 0);
        assert_eq!(normalized_to_screen(-3.0, -3.0, &m), (0, 0));
        assert_eq!(normalized_to_screen(9.0, 9.0, &m), (1536, 864));
    }

    /// Regression guard for the y/x ordering, which is the single easiest
    /// thing to get backwards: Gemini's point is [y, x], NOT [x, y]. The
    /// real observed value from a failing YouTube run was [146, 396], which
    /// must mean "near the top, left of centre" — swapping the two would
    /// put it far down the right-hand side instead.
    #[test]
    fn gemini_point_is_y_then_x_not_x_then_y() {
        let m = monitor(1536, 864, 0, 0);
        let (x, y) = gemini_point_to_screen(146.0, 396.0, &m);
        assert_eq!((x, y), (608, 126));
        assert!(y < m.height_px as i32 / 4, "y should be in the top quarter, got {}", y);
        assert!(x < m.width_px as i32 / 2, "x should be left of centre, got {}", x);
    }

    /// The test that actually answers "where does the mouse land": performs
    /// the SAME cursor move execute_click uses, then reads the real OS
    /// cursor back via GetCursorPos and compares. Catches any mismatch
    /// between our coordinate space and what the OS actually does (DPI
    /// virtualization, monitor origin errors, and pointer-speed scaling of
    /// injected relative moves) — none of which a pure-math test can see.
    ///
    /// This test earned its keep immediately: against the previous
    /// enigo relative-move implementation it failed with target (760, 157)
    /// vs landed (1528, 312), exposing that Windows scales injected
    /// MOUSEEVENTF_MOVE deltas by the user's pointer-speed setting.
    #[test]
    fn relative_move_lands_the_real_cursor_where_intended() {
        // Serialized against dpi_tests, which also moves the real cursor.
        let _guard = crate::dpi_tests::CURSOR_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::set_dpi_awareness();

        let (_, m) = crate::capture::cursor::get_cursor_and_monitor()
            .expect("failed to read cursor/monitor");
        println!(
            "monitor {}x{}@{} dpi, origin ({}, {})",
            m.width_px, m.height_px, m.dpi, m.origin_x, m.origin_y
        );

        // Includes the real [146, 396] from the failing YouTube run.
        let points: [(f32, f32); 5] = [
            (146.0, 396.0),
            (500.0, 500.0),
            (100.0, 100.0),
            (900.0, 800.0),
            (250.0, 750.0),
        ];
        let tolerance = 2; // px, for f32 truncation

        for (py, px) in points {
            let (target_x, target_y) = gemini_point_to_screen(py, px, &m);

            // Retried, because this reads the one real system cursor: if a
            // human happens to move the physical mouse during the window
            // between setting and reading, the position legitimately
            // differs by a few px. A systematic bug (the pointer-speed
            // scaling this test was written to catch) misses by hundreds
            // and fails every attempt, so retrying can't mask one.
            let mut landed = POINT::default();
            const ATTEMPTS: i32 = 6;
            for attempt in 0..ATTEMPTS {
                // Park the cursor somewhere unrelated first, so a passing
                // assertion can't be an artifact of it already being there.
                unsafe { SetCursorPos(m.origin_x + 5, m.origin_y + 5).expect("SetCursorPos failed") };

                // Same call execute_click makes.
                unsafe { SetCursorPos(target_x, target_y).expect("SetCursorPos failed") };
                sleep(Duration::from_millis(40));

                unsafe { GetCursorPos(&mut landed).expect("GetCursorPos failed") };
                let close_enough = (landed.x - target_x).abs() <= tolerance
                    && (landed.y - target_y).abs() <= tolerance;
                if close_enough {
                    break;
                }
                if attempt < ATTEMPTS - 1 {
                    println!(
                        "  retry {}: landed ({}, {}) — likely live mouse movement",
                        attempt + 1, landed.x, landed.y
                    );
                    sleep(Duration::from_millis(120));
                }
            }

            println!(
                "gemini [y={}, x={}] -> target ({}, {}) -> landed ({}, {})  delta ({}, {})",
                py, px, target_x, target_y, landed.x, landed.y,
                landed.x - target_x, landed.y - target_y
            );

            assert!(
                (landed.x - target_x).abs() <= tolerance,
                "x off by {}: target {}, landed {}", (landed.x - target_x).abs(), target_x, landed.x
            );
            assert!(
                (landed.y - target_y).abs() <= tolerance,
                "y off by {}: target {}, landed {}", (landed.y - target_y).abs(), target_y, landed.y
            );
        }
    }
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
