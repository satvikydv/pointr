pub mod capture;
pub mod overlay;
pub mod commands;

use std::sync::Mutex;
use crate::capture::cursor::MonitorInfo;

pub struct CaptureState {
    pub image_bytes: Vec<u8>,
    pub monitor: Option<MonitorInfo>,
    pub cursor_norm: Option<(f32, f32)>,
    pub active_window_title: String,
}

/// Shared first step for both hotkey paths: where's the cursor, which monitor
/// is it on, and what does that monitor look like right now.
fn capture_now() -> anyhow::Result<(Vec<u8>, MonitorInfo, (f32, f32), &'static str)> {
    let (pos, monitor) = capture::cursor::get_cursor_and_monitor()?;
    let (image_bytes, method) = capture::screen::capture_monitor(&monitor)?;
    let method_str = match method {
        capture::screen::CaptureMethod::Wgc => "WGC",
        capture::screen::CaptureMethod::Gdi => "GDI",
    };
    let coords = capture::coords::normalize_cursor(pos.x, pos.y, &monitor);
    Ok((image_bytes, monitor, (coords.x_norm, coords.y_norm), method_str))
}

/// Secondary (region-select) hotkey path — unchanged behavior: capture the
/// monitor, show the full-screenshot overlay, let the user drag a crop box.
#[tauri::command]
fn trigger_capture(app: tauri::AppHandle, state: tauri::State<'_, Mutex<CaptureState>>) -> Result<String, String> {
    let (image_bytes, monitor, cursor_norm, method_str) = capture_now()
        .map_err(|e| format!("Capture failed: {}", e))?;
    let image_bytes = capture::resize::cap_long_edge(&image_bytes)
        .map_err(|e| format!("Failed to resize capture: {}", e))?;
    let active_window_title = capture::context::get_foreground_app_context().describe();

    {
        let mut state_lock = state.lock().unwrap();
        state_lock.image_bytes = image_bytes.clone();
        state_lock.monitor = Some(monitor.clone());
        state_lock.cursor_norm = Some(cursor_norm);
        state_lock.active_window_title = active_window_title;
    }

    overlay::window::show_overlay(&app, &monitor, &image_bytes)
        .map_err(|e| format!("Failed to show overlay: {}", e))?;

    Ok(format!(
        "Captured {} bytes via {}. Monitor {}x{}@{}, origin ({}, {})",
        image_bytes.len(),
        method_str,
        monitor.width_px,
        monitor.height_px,
        monitor.dpi,
        monitor.origin_x,
        monitor.origin_y
    ))
}

/// Primary hotkey path: capture, burn a marker into the image at the actual
/// cursor position, and hand off to the frontend to send it straight to the
/// backend — no region selection in the loop.
fn trigger_capture_direct(app: &tauri::AppHandle, state: &tauri::State<'_, Mutex<CaptureState>>) -> Result<String, String> {
    let (image_bytes, monitor, cursor_norm, method_str) = capture_now()
        .map_err(|e| format!("Capture failed: {}", e))?;
    // Read this now, not after the overlay is shown — once show_overlay_direct
    // focuses our own window below, GetForegroundWindow would report Pointr
    // itself instead of whatever the user was actually looking at.
    let active_window_title = capture::context::get_foreground_app_context().describe();

    let cursor_px_x = (cursor_norm.0 * monitor.width_px as f32) as i64;
    let cursor_px_y = (cursor_norm.1 * monitor.height_px as f32) as i64;
    let marked_bytes = capture::marker::draw_cursor_marker(&image_bytes, cursor_px_x, cursor_px_y)
        .map_err(|e| format!("Failed to draw cursor marker: {}", e))?;
    // Resize after the marker is burned in — cursor_px_x/y are computed
    // against the full-resolution monitor dimensions, so the marker must be
    // drawn there first; a uniform resize afterward keeps it in the same
    // relative position, just smaller.
    let marked_bytes = capture::resize::cap_long_edge(&marked_bytes)
        .map_err(|e| format!("Failed to resize capture: {}", e))?;

    {
        let mut state_lock = state.lock().unwrap();
        state_lock.image_bytes = marked_bytes.clone();
        state_lock.monitor = Some(monitor.clone());
        state_lock.cursor_norm = Some(cursor_norm);
        state_lock.active_window_title = active_window_title;
    }

    overlay::window::show_overlay_direct(app, &monitor)
        .map_err(|e| format!("Failed to show overlay: {}", e))?;

    Ok(format!(
        "Captured {} bytes via {} (direct mode). Monitor {}x{}@{}",
        marked_bytes.len(),
        method_str,
        monitor.width_px,
        monitor.height_px,
        monitor.dpi,
    ))
}

use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::GlobalShortcutExt;

fn escape_shortcut() -> tauri_plugin_global_shortcut::Shortcut {
    use tauri_plugin_global_shortcut::{Code, Shortcut};
    Shortcut::new(None, Code::Escape)
}

/// Click-through windows (`setIgnoreCursorEvents(true)`, used while the answer
/// is shown so the overlay never blocks the app underneath) are unreliable
/// about keeping OS keyboard focus on Windows — the DOM `keydown` listener for
/// Escape can silently stop firing. Routed through the global-shortcut plugin
/// instead, since that's how the hotkeys already work regardless of focus.
/// Registered only while the overlay is actually showing a response, so it
/// doesn't swallow Escape presses meant for other apps the rest of the time.
#[tauri::command]
fn enable_escape_dismiss(app: tauri::AppHandle) -> Result<(), String> {
    app.global_shortcut()
        .register(escape_shortcut())
        .map_err(|e| format!("Failed to register Escape: {}", e))
}

#[tauri::command]
fn disable_escape_dismiss(app: tauri::AppHandle) -> Result<(), String> {
    app.global_shortcut()
        .unregister(escape_shortcut())
        .map_err(|e| format!("Failed to unregister Escape: {}", e))
}

/// Without this, the process defaults to DPI-unaware on Windows, which makes
/// `GetCursorPos` (capture/cursor.rs) return coordinates virtualized/scaled
/// to 96 DPI — while `GetMonitorInfoW`'s monitor rect (used for the same
/// normalization) is always real physical pixels. At any scaling other than
/// 100% that mismatch puts the cursor marker in the wrong place (e.g. at
/// 125%/120 DPI, systematically shifted toward the top-left). Must be called
/// before any window is created, so this runs first thing in `run()`.
fn set_dpi_awareness() {
    use windows::Win32::UI::HiDpi::{
        SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    };
    unsafe {
        if let Err(e) = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) {
            eprintln!("Failed to set per-monitor DPI awareness: {}", e);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    set_dpi_awareness();

    tauri::Builder::default()
        .manage(Mutex::new(CaptureState {
            image_bytes: Vec::new(),
            monitor: None,
            cursor_norm: None,
            active_window_title: String::new(),
        }))
        .invoke_handler(tauri::generate_handler![
            trigger_capture,
            commands::analyze::process_crop,
            commands::analyze::process_direct,
            enable_escape_dismiss,
            disable_escape_dismiss
        ])
        .setup(|app| {
            use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};
            // Primary: single press, immediate capture + analyze, no region select.
            let primary = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space);
            // Secondary: manual drag-to-crop flow, demoted behind an extra modifier.
            let region_select = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT), Code::Space);
            let escape = escape_shortcut();

            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |_app, shortcut, event| {
                        if event.state() != ShortcutState::Pressed {
                            return;
                        }
                        if shortcut == &primary {
                            println!("Primary hotkey pressed — direct capture + analyze...");
                            let state = _app.state::<Mutex<CaptureState>>();
                            match trigger_capture_direct(_app, &state) {
                                Ok(res) => println!("{}", res),
                                Err(e) => eprintln!("Direct capture error: {}", e),
                            }
                        } else if shortcut == &region_select {
                            println!("Region-select hotkey pressed — manual crop flow...");
                            let state = _app.state::<Mutex<CaptureState>>();
                            match trigger_capture(_app.clone(), state) {
                                Ok(res) => println!("{}", res),
                                Err(e) => eprintln!("Capture error: {}", e),
                            }
                        } else if shortcut == &escape {
                            // Only registered while an answer is showing (see
                            // enable_escape_dismiss) — just tell the frontend
                            // to run its existing dismiss path.
                            let _ = _app.emit("dismiss-overlay", ());
                        }
                    })
                    .build(),
            )?;

            app.global_shortcut().register(primary)?;
            app.global_shortcut().register(region_select)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod dpi_tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

    /// Regression test for the DPI-awareness bug: moves the real system
    /// cursor to known fractional positions on the current monitor (whatever
    /// DPI scaling this machine actually has) and asserts our own
    /// get_cursor_and_monitor + normalize_cursor pipeline reports those same
    /// fractions back. Without set_dpi_awareness(), GetCursorPos returns
    /// coordinates virtualized to 96 DPI while the monitor rect stays
    /// physical, so at any scaling != 100% this would fail by roughly the
    /// scale-factor ratio (e.g. ~0.8x at 125%) — well outside the tolerance.
    #[test]
    fn cursor_normalization_matches_at_current_dpi_scaling() {
        set_dpi_awareness();

        let (_, monitor) = capture::cursor::get_cursor_and_monitor()
            .expect("failed to read initial cursor/monitor");
        println!(
            "Testing against monitor {}x{}@{} DPI, origin ({}, {})",
            monitor.width_px, monitor.height_px, monitor.dpi, monitor.origin_x, monitor.origin_y
        );

        let test_fractions: [(f32, f32); 4] = [(0.1, 0.1), (0.5, 0.5), (0.9, 0.75), (0.25, 0.6)];
        let tolerance = 0.02; // 2% of screen dimension

        for (x_frac, y_frac) in test_fractions {
            let target_x = monitor.origin_x + (x_frac * monitor.width_px as f32) as i32;
            let target_y = monitor.origin_y + (y_frac * monitor.height_px as f32) as i32;

            unsafe {
                SetCursorPos(target_x, target_y).expect("SetCursorPos failed");
            }

            let (pos, monitor_after) = capture::cursor::get_cursor_and_monitor()
                .expect("failed to read cursor/monitor after move");
            let coords = capture::coords::normalize_cursor(pos.x, pos.y, &monitor_after);

            println!(
                "target=({:.2},{:.2}) -> physical=({}, {}) -> normalized=({:.4},{:.4})",
                x_frac, y_frac, pos.x, pos.y, coords.x_norm, coords.y_norm
            );

            assert!(
                (coords.x_norm - x_frac).abs() < tolerance,
                "x_norm mismatch at target ({}, {}): got {}, expected ~{} (tolerance {})",
                x_frac, y_frac, coords.x_norm, x_frac, tolerance
            );
            assert!(
                (coords.y_norm - y_frac).abs() < tolerance,
                "y_norm mismatch at target ({}, {}): got {}, expected ~{} (tolerance {})",
                x_frac, y_frac, coords.y_norm, y_frac, tolerance
            );
        }
    }
}
