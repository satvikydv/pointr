pub mod api_config;
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
    pub app_name: String,
    pub session_id: String,
    pub session_duration_secs: f64,
    /// HWND of the window that was foreground at capture time — restored
    /// before an agent action types into "the focused field", since by
    /// execution time Pointr's own window has OS focus instead. See
    /// capture::context::AppContext::hwnd for why this needs to be captured
    /// this early rather than looked up fresh at execution time.
    pub target_hwnd: isize,
}

/// One app's ongoing session: when it started, and when it was last touched
/// (used to decide whether returning to this app resumes it or starts over).
struct AppSession {
    session_id: String,
    started_at: std::time::Instant,
    last_active_at: std::time::Instant,
}

const SESSION_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);
/// Safety cap on distinct apps tracked at once — realistically dozens at
/// most in a single run, but bounds memory if something churns through many.
const MAX_TRACKED_APPS: usize = 50;

/// Keyed by app_name, so switching away and back (e.g. Chrome -> VS Code ->
/// Chrome) resumes the same session_id and dwell-time clock for VS Code
/// rather than starting a fresh one — as long as it hasn't been more than
/// SESSION_IDLE_TIMEOUT since that app was last active, in which case it's
/// treated as stale and a new session starts. Backend builds up a short
/// history of recent Q&A per session_id instead of treating every capture as
/// a cold start (see session_memory.py).
struct SessionState {
    sessions: std::collections::HashMap<String, AppSession>,
}

impl SessionState {
    fn new() -> Self {
        Self {
            sessions: std::collections::HashMap::new(),
        }
    }

    /// Returns (session_id, seconds since this app's session started).
    fn resolve(&mut self, current_app_name: &str) -> (String, f64) {
        let now = std::time::Instant::now();

        let expired = match self.sessions.get(current_app_name) {
            Some(s) => now.duration_since(s.last_active_at) > SESSION_IDLE_TIMEOUT,
            None => true,
        };

        if expired {
            if self.sessions.len() >= MAX_TRACKED_APPS {
                if let Some(oldest_key) = self
                    .sessions
                    .iter()
                    .min_by_key(|(_, s)| s.last_active_at)
                    .map(|(k, _)| k.clone())
                {
                    self.sessions.remove(&oldest_key);
                }
            }
            self.sessions.insert(
                current_app_name.to_string(),
                AppSession {
                    session_id: uuid::Uuid::new_v4().to_string(),
                    started_at: now,
                    last_active_at: now,
                },
            );
        } else if let Some(s) = self.sessions.get_mut(current_app_name) {
            s.last_active_at = now;
        }

        let s = self.sessions.get(current_app_name).unwrap();
        (s.session_id.clone(), s.started_at.elapsed().as_secs_f64())
    }
}

/// Shared first step for both hotkey paths: where's the cursor, which monitor
/// is it on, and what does that monitor look like right now. Returns the raw
/// decoded frame, not PNG bytes — see `capture::screen::capture_monitor`.
fn capture_now() -> anyhow::Result<(image::RgbaImage, MonitorInfo, (f32, f32), &'static str)> {
    let (pos, monitor) = capture::cursor::get_cursor_and_monitor()?;
    let (img, method) = capture::screen::capture_monitor(&monitor)?;
    let method_str = match method {
        capture::screen::CaptureMethod::Wgc => "WGC",
        capture::screen::CaptureMethod::Gdi => "GDI",
    };
    let coords = capture::coords::normalize_cursor(pos.x, pos.y, &monitor);
    Ok((img, monitor, (coords.x_norm, coords.y_norm), method_str))
}

/// Secondary (region-select) hotkey path — unchanged behavior: capture the
/// monitor, show the full-screenshot overlay, let the user drag a crop box.
#[tauri::command]
fn trigger_capture(
    app: tauri::AppHandle,
    state: tauri::State<'_, Mutex<CaptureState>>,
    session: tauri::State<'_, Mutex<SessionState>>,
) -> Result<String, String> {
    let (img, monitor, cursor_norm, method_str) = capture_now()
        .map_err(|e| format!("Capture failed: {}", e))?;
    let resized = capture::resize::cap_long_edge(&img);
    let image_bytes = capture::resize::encode_png(&resized)
        .map_err(|e| format!("Failed to encode capture: {}", e))?;
    let ctx = capture::context::get_foreground_app_context();
    let (session_id, session_duration_secs) = session.lock().unwrap().resolve(&ctx.app_name);

    {
        let mut state_lock = state.lock().unwrap();
        state_lock.image_bytes = image_bytes.clone();
        state_lock.monitor = Some(monitor.clone());
        state_lock.cursor_norm = Some(cursor_norm);
        state_lock.active_window_title = ctx.describe();
        state_lock.app_name = ctx.app_name;
        state_lock.session_id = session_id;
        state_lock.session_duration_secs = session_duration_secs;
        state_lock.target_hwnd = ctx.hwnd;
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
fn trigger_capture_direct(
    app: &tauri::AppHandle,
    state: &tauri::State<'_, Mutex<CaptureState>>,
    session: &tauri::State<'_, Mutex<SessionState>>,
) -> Result<String, String> {
    let (mut img, monitor, cursor_norm, method_str) = capture_now()
        .map_err(|e| format!("Capture failed: {}", e))?;
    // Read this now, not after the overlay is shown — once show_overlay_direct
    // focuses our own window below, GetForegroundWindow would report Pointr
    // itself instead of whatever the user was actually looking at.
    let ctx = capture::context::get_foreground_app_context();
    let (session_id, session_duration_secs) = session.lock().unwrap().resolve(&ctx.app_name);

    let cursor_px_x = (cursor_norm.0 * monitor.width_px as f32) as i64;
    let cursor_px_y = (cursor_norm.1 * monitor.height_px as f32) as i64;
    // Marker drawn against the full-resolution frame (before resize) so
    // cursor_px_x/y — computed from the full-res monitor dimensions — land in
    // the right place; a uniform resize afterward keeps it proportionally
    // correct, just smaller.
    capture::marker::draw_cursor_marker(&mut img, cursor_px_x, cursor_px_y);
    let resized = capture::resize::cap_long_edge(&img);
    let marked_bytes = capture::resize::encode_png(&resized)
        .map_err(|e| format!("Failed to encode capture: {}", e))?;

    {
        let mut state_lock = state.lock().unwrap();
        state_lock.image_bytes = marked_bytes.clone();
        state_lock.monitor = Some(monitor.clone());
        state_lock.cursor_norm = Some(cursor_norm);
        state_lock.active_window_title = ctx.describe();
        state_lock.app_name = ctx.app_name;
        state_lock.session_id = session_id;
        state_lock.session_duration_secs = session_duration_secs;
        state_lock.target_hwnd = ctx.hwnd;
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
            app_name: String::new(),
            session_id: String::new(),
            session_duration_secs: 0.0,
            target_hwnd: 0,
        }))
        .manage(Mutex::new(SessionState::new()))
        .manage(commands::tts::TtsState::new())
        .invoke_handler(tauri::generate_handler![
            api_config::get_api_config,
            trigger_capture,
            commands::analyze::process_crop,
            commands::analyze::process_direct,
            commands::analyze::process_explain,
            commands::analyze::get_current_screenshot_base64,
            commands::analyze::get_active_window_title,
            commands::analyze::capture_fresh_screenshot,
            commands::clipboard::read_clipboard,
            commands::clipboard::write_clipboard,
            commands::tts::speak_text,
            commands::tts::stop_speech,
            commands::settings::list_voices,
            commands::settings::get_selected_voice,
            commands::settings::set_selected_voice,
            commands::settings::get_speech_enabled,
            commands::settings::set_speech_enabled,
            commands::settings::get_os_actions_enabled,
            commands::settings::set_os_actions_enabled,
            commands::settings::save_github_token,
            commands::settings::get_github_token_status,
            commands::settings::clear_github_token,
            commands::settings::get_github_token_for_request,
            commands::settings::save_gemini_key,
            commands::settings::get_gemini_key_status,
            commands::settings::clear_gemini_key,
            commands::settings::get_gemini_key_for_request,
            commands::settings::save_tavily_key,
            commands::settings::get_tavily_key_status,
            commands::settings::clear_tavily_key,
            commands::settings::get_tavily_key_for_request,
            commands::actions::execute_type_text,
            commands::actions::execute_open_app,
            commands::actions::execute_click,
            commands::actions::execute_key_press,
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
                            let session = _app.state::<Mutex<SessionState>>();
                            match trigger_capture_direct(_app, &state, &session) {
                                Ok(res) => println!("{}", res),
                                Err(e) => eprintln!("Direct capture error: {}", e),
                            }
                        } else if shortcut == &region_select {
                            println!("Region-select hotkey pressed — manual crop flow...");
                            let state = _app.state::<Mutex<CaptureState>>();
                            let session = _app.state::<Mutex<SessionState>>();
                            match trigger_capture(_app.clone(), state, session) {
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

            // Tray icon: the only way to reach Settings or quit cleanly,
            // since the overlay window itself is borderless/hidden-by-default
            // and there was previously no exit path other than Task Manager.
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;

            let settings_item = MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Pointr", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&settings_item, &quit_item])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(win) = app.get_webview_window("settings") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

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
