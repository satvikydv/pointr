//! Coding questions shown on screen (experimental, meeting mode only).
//!
//! A coding problem in an interview is usually shown, not spoken: in a
//! shared screen inside the meeting app, or on a coding site. During a
//! meeting session, Ctrl+Alt+S (or the first free fallback key):
//!
//! 1. Captures the screen right away, cropped to the foreground window when
//!    that helps (see code_util.rs), at up to 2400 px so small text stays
//!    readable.
//! 2. Sends it with the last couple of minutes of transcript (constraints
//!    are often added out loud), the preferred language, the meeting
//!    background and the previous solution, if any, to
//!    `/api/meeting/code-stream`, using the separate, stronger **coding
//!    model** from Settings. Spoken answers (Ctrl+Alt+Enter) keep their own
//!    fast model and never wait on this.
//! 3. Streams the answer into the code panel, a tall window on the right of
//!    the screen that is always excluded from capture, never takes focus and
//!    lets clicks through. The small meeting panel mirrors the Problem and
//!    Approach lines so the user can start talking.
//!
//! Pressing again revises: the previous solution goes along with the new
//! screenshot, so "now make it O(n)" or new code in the editor is taken
//! into account. The previous solution is forgotten when the session ends.
//!
//! Keys, registered only for a session: the solve key, Ctrl+Alt+X to hide
//! or show the code panel, Ctrl+Alt+PageUp/PageDown to scroll it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine;
use futures_util::StreamExt;
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

use super::code_util::{self, Rect};
use super::Ctrl;
use crate::api_config::{API_BASE_URL, CLIENT_KEY};
use crate::stealth::{self, Protection};

/// How much recent speech goes along: interviewers add constraints out
/// loud ("assume it's sorted"), but the problem itself is on screen.
const TRANSCRIPT_SECS: f64 = 2.0 * 60.0;
const TRANSCRIPT_MAX_CHARS: usize = 4_000;

/// Longest wait for the phrase in progress to be transcribed.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

/// Bumped by every solve and by `reset`; a stream that's no longer current
/// stops, so pressing again or ending the meeting supersedes it.
static CURRENT: AtomicU64 = AtomicU64::new(0);

/// The last finished solution in this session, sent with the next press so
/// it revises rather than starts over.
static LAST_SOLUTION: Mutex<Option<String>> = Mutex::new(None);

static PANEL_VISIBLE: AtomicBool = AtomicBool::new(false);

fn is_current(id: u64) -> bool {
    CURRENT.load(Ordering::SeqCst) == id
}

// ---------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Solve,
    TogglePanel,
    Scroll(i32),
}

const CTRL_ALT: Modifiers = Modifiers::CONTROL.union(Modifiers::ALT);

/// Tried in order: another app may already own the first (as happened with
/// Ctrl+Alt+M). None of these clash with the meeting keys.
const SOLVE_CANDIDATES: [(Code, &str); 3] =
    [(Code::KeyS, "Ctrl+Alt+S"), (Code::KeyK, "Ctrl+Alt+K"), (Code::Semicolon, "Ctrl+Alt+;")];

/// What's registered for this session, for `action_for` and `unregister`.
static KEYS: Mutex<Vec<(Shortcut, Action)>> = Mutex::new(Vec::new());
static SOLVE_KEY: Mutex<Option<&'static str>> = Mutex::new(None);

/// The solve key while a session runs, if one could be registered.
pub fn solve_key() -> Option<&'static str> {
    *SOLVE_KEY.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers the coding keys for a session. Never fails the session: a key
/// another app owns is skipped and logged.
pub fn register(app: &AppHandle) {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    for (code, label) in SOLVE_CANDIDATES {
        let shortcut = Shortcut::new(Some(CTRL_ALT), code);
        match app.global_shortcut().register(shortcut) {
            Ok(()) => {
                keys.push((shortcut, Action::Solve));
                *SOLVE_KEY.lock().unwrap_or_else(|e| e.into_inner()) = Some(label);
                break;
            }
            Err(e) => eprintln!("[code] couldn't register {}: {}", label, e),
        }
    }
    for (code, action) in [
        (Code::KeyX, Action::TogglePanel),
        (Code::PageUp, Action::Scroll(-1)),
        (Code::PageDown, Action::Scroll(1)),
    ] {
        let shortcut = Shortcut::new(Some(CTRL_ALT), code);
        match app.global_shortcut().register(shortcut) {
            Ok(()) => keys.push((shortcut, action)),
            Err(e) => eprintln!("[code] couldn't register {:?}: {}", action, e),
        }
    }
    drop(keys);
    let _ = app.emit("code-keys", serde_json::json!({ "solve_key": solve_key() }));
}

pub fn unregister(app: &AppHandle) {
    for (shortcut, _) in KEYS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = app.global_shortcut().unregister(shortcut);
    }
    *SOLVE_KEY.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub fn action_for(shortcut: &Shortcut) -> Option<Action> {
    KEYS.lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|(s, _)| s == shortcut)
        .map(|(_, a)| *a)
}

/// Runs off the shortcut handler's thread (see lib.rs): showing the panel
/// and the capture must not hold up the shortcut plugin.
pub fn handle(app: &AppHandle, action: Action) {
    match action {
        Action::Solve => solve_now(app),
        Action::TogglePanel => {
            if PANEL_VISIBLE.load(Ordering::SeqCst) {
                hide_panel(app);
            } else {
                show_panel(app);
            }
        }
        Action::Scroll(dir) => {
            let _ = app.emit_to(stealth::CODE_WINDOW, "code-scroll", dir);
        }
    }
}

/// End of a session: cancel any solve in flight, forget the last solution,
/// hide the panel.
pub fn reset(app: &AppHandle) {
    CURRENT.fetch_add(1, Ordering::SeqCst);
    *LAST_SOLUTION.lock().unwrap_or_else(|e| e.into_inner()) = None;
    hide_panel(app);
    let _ = app.emit_to(stealth::CODE_WINDOW, "code-reset", ());
}

// ---------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------

fn panel(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(stealth::CODE_WINDOW)
}

/// Run once at startup: click-through, like the meeting panel.
pub fn setup_panel(app: &AppHandle) {
    if let Some(window) = panel(app) {
        let _ = window.set_ignore_cursor_events(true);
    }
}

/// Docks the panel to the right edge of the monitor under the cursor, so it
/// sits beside whatever is being shared rather than on top of it.
fn place(app: &AppHandle, window: &WebviewWindow) {
    let Ok((pos, _)) = crate::capture::cursor::get_cursor_and_monitor() else { return };
    let monitor = app
        .available_monitors()
        .ok()
        .and_then(|ms| {
            ms.into_iter().find(|m| {
                let (p, s) = (m.position(), m.size());
                pos.x >= p.x && pos.x < p.x + s.width as i32 && pos.y >= p.y && pos.y < p.y + s.height as i32
            })
        })
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let (p, s, scale) = (monitor.position(), monitor.size(), monitor.scale_factor());
    let width = ((s.width as f64 * 0.36) as u32).clamp((420.0 * scale) as u32, (720.0 * scale) as u32);
    let height = (s.height as f64 * 0.84) as u32;
    let margin = (12.0 * scale) as i32;
    let _ = window.set_size(tauri::PhysicalSize::new(width, height));
    let _ = window.set_position(tauri::PhysicalPosition::new(
        p.x + s.width as i32 - width as i32 - margin,
        p.y + (s.height as i32 - height as i32) / 2,
    ));
}

/// Same guarantee as the meeting panel: never shown if this Windows build
/// can't keep it out of capture, and hidden again straight away if its
/// protection didn't apply.
fn show_panel(app: &AppHandle) {
    let Some(window) = panel(app) else { return };
    if stealth::panel_protection() == Protection::Failed {
        eprintln!("[code] panel not shown: this Windows build can't keep it out of screen capture");
        return;
    }
    if !PANEL_VISIBLE.load(Ordering::SeqCst) {
        place(app, &window);
    }
    stealth::show_without_focus(&window);
    if !stealth::is_protected(&window) {
        stealth::hide(&window);
        PANEL_VISIBLE.store(false, Ordering::SeqCst);
        eprintln!("[code] panel hidden again: its capture protection wasn't applied");
        return;
    }
    PANEL_VISIBLE.store(true, Ordering::SeqCst);
}

fn hide_panel(app: &AppHandle) {
    if let Some(window) = panel(app) {
        stealth::hide(&window);
    }
    PANEL_VISIBLE.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------

/// The foreground window's visible bounds (without the drop shadow).
fn foreground_rect() -> Option<Rect> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut r = RECT::default();
        let ok = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
            || GetWindowRect(hwnd, &mut r).is_ok();
        ok.then_some(Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
    }
}

/// Screenshot for the model, as base64 PNG, plus whether it was cropped.
fn capture_problem() -> Result<(String, bool), String> {
    let (_, monitor) =
        crate::capture::cursor::get_cursor_and_monitor().map_err(|e| format!("Couldn't find the screen: {}", e))?;
    let (img, _) = crate::capture::screen::capture_monitor(&monitor)
        .map_err(|e| format!("Couldn't capture the screen: {}", e))?;
    let crop = foreground_rect().and_then(|r| {
        code_util::crop_for(r, (monitor.origin_x, monitor.origin_y), (monitor.width_px, monitor.height_px))
    });
    let img = match crop {
        Some((x, y, w, h)) => image::imageops::crop_imm(&img, x, y, w, h).to_image(),
        None => img,
    };
    let (w, h) = code_util::fit_long_edge(img.width(), img.height(), code_util::CODE_MAX_LONG_EDGE);
    let img = if (w, h) == img.dimensions() {
        img
    } else {
        image::DynamicImage::ImageRgba8(img)
            .resize_exact(w, h, image::imageops::FilterType::Lanczos3)
            .to_rgba8()
    };
    let png = crate::capture::resize::encode_png(&img).map_err(|e| format!("Couldn't encode the screenshot: {}", e))?;
    Ok((base64::engine::general_purpose::STANDARD.encode(png), crop.is_some()))
}

// ---------------------------------------------------------------------
// Solve
// ---------------------------------------------------------------------

#[derive(Clone, serde::Serialize)]
struct Start {
    id: u64,
    language: String,
    revising: bool,
}

#[derive(Clone, serde::Serialize)]
struct Chunk {
    id: u64,
    text: String,
}

#[derive(Clone, serde::Serialize)]
struct Done {
    id: u64,
    text: String,
    first_token_ms: Option<u64>,
    total_ms: u64,
}

#[derive(Clone, serde::Serialize)]
struct Failure {
    id: u64,
    message: String,
}

fn fail(app: &AppHandle, id: u64, message: impl Into<String>) {
    if is_current(id) {
        let _ = app.emit("code-error", Failure { id, message: message.into() });
    }
}

pub fn solve_now(app: &AppHandle) {
    let id = CURRENT.fetch_add(1, Ordering::SeqCst) + 1;
    let pressed = Instant::now();

    // Capture first, before anything on screen can change (including the
    // panels appearing, though they're excluded from capture anyway).
    let capture = capture_problem();

    let language = crate::commands::settings::coding_language(app);
    let previous = LAST_SOLUTION.lock().unwrap_or_else(|e| e.into_inner()).clone();
    show_panel(app);
    super::reveal_panel(app);
    let _ = app.emit("code-start", Start { id, language: language.clone(), revising: previous.is_some() });

    let (screenshot, cropped) = match capture {
        Ok(c) => c,
        Err(e) => return fail(app, id, e),
    };
    let handles = super::session_handles();

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // The phrase being spoken right now, into the transcript.
        let transcript = match handles {
            Some((ctrl, transcript)) => {
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    let (tx, rx) = mpsc::channel();
                    if ctrl.send(Ctrl::Flush(tx)).is_ok() {
                        let _ = rx.recv_timeout(FLUSH_TIMEOUT);
                    }
                })
                .await;
                transcript
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .recent(TRANSCRIPT_SECS, TRANSCRIPT_MAX_CHARS)
            }
            None => Vec::new(),
        };
        if !is_current(id) {
            return;
        }
        if cfg!(debug_assertions) {
            eprintln!(
                "[code] solve {}: screenshot {} KB ({}), {} transcript line(s), {}, captured+flushed in {} ms",
                id,
                screenshot.len() * 3 / 4 / 1024,
                if cropped { "foreground window" } else { "whole monitor" },
                transcript.len(),
                if previous.is_some() { "revising" } else { "new" },
                pressed.elapsed().as_millis()
            );
        }

        let lines: Vec<_> = transcript
            .iter()
            .map(|l| serde_json::json!({ "t": (l.t * 10.0).round() / 10.0, "text": l.text }))
            .collect();
        let mut body = serde_json::json!({
            "screenshots": [screenshot],
            "transcript": lines,
            "language": language,
            "previous_solution": previous.unwrap_or_default(),
            "brief": crate::commands::settings::meeting_brief(&app),
            "gemini_api_key": crate::commands::settings::get_gemini_key_for_request(app.clone()).unwrap_or_default(),
        });
        if let (Some(target), serde_json::Value::Object(fields)) =
            (body.as_object_mut(), crate::commands::settings::coding_request_fields(&app))
        {
            target.extend(fields);
        }

        if let Err(e) = stream(&app, id, body, pressed).await {
            fail(&app, id, e);
        }
    });
}

async fn stream(app: &AppHandle, id: u64, body: serde_json::Value, pressed: Instant) -> Result<(), String> {
    let res = reqwest::Client::new()
        .post(format!("{}/api/meeting/code-stream", API_BASE_URL))
        .header("X-Pointr-Client-Key", CLIENT_KEY)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the Pointr backend: {}", e))?;
    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("detail").and_then(|d| d.as_str()).map(str::to_string))
            .unwrap_or(text);
        return Err(format!("Backend error ({}): {}", status, detail));
    }

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    let mut first_token_ms = None;
    while let Some(chunk) = stream.next().await {
        if !is_current(id) {
            return Ok(()); // superseded; dropping the stream closes it
        }
        let bytes = chunk.map_err(|e| format!("The solution stream broke off: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
            match event.get("type").and_then(|v| v.as_str()) {
                Some("chunk") => {
                    if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                        first_token_ms.get_or_insert(pressed.elapsed().as_millis() as u64);
                        let _ = app.emit("code-chunk", Chunk { id, text: text.to_string() });
                    }
                }
                Some("done") => {
                    let text = event.get("answer_text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !text.is_empty() {
                        *LAST_SOLUTION.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
                    }
                    let total_ms = pressed.elapsed().as_millis() as u64;
                    if cfg!(debug_assertions) {
                        eprintln!("[code] solve {}: first token {:?} ms, done {} ms", id, first_token_ms, total_ms);
                    }
                    let _ = app.emit("code-done", Done { id, text, first_token_ms, total_ms });
                }
                _ => {}
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct CodeStatus {
    solve_key: Option<&'static str>,
    protection: Protection,
}

#[tauri::command]
pub fn get_code_status() -> CodeStatus {
    CodeStatus { solve_key: solve_key(), protection: stealth::panel_protection() }
}
