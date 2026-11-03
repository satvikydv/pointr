//! Ctrl+Alt+Enter: turn the last few minutes of the meeting into a
//! suggested answer, streamed into the meeting panel as it's written.
//!
//! The backend route (`/api/meeting/answer-stream`) speaks the same NDJSON
//! as the screen-answer stream: `{"type":"chunk","text":...}` lines, then
//! one `{"type":"done","answer_text":...}`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tauri::{AppHandle, Emitter};

use super::transcript::Line;
use super::Ctrl;
use crate::api_config::{API_BASE_URL, CLIENT_KEY};

/// How far back the answer looks. Long enough for a question with some
/// set-up before it, short enough to stay on the current topic.
const RECENT_SECS: f64 = 4.0 * 60.0;
const RECENT_MAX_CHARS: usize = 8_000;

/// Longest wait for the last phrase to be transcribed before answering
/// from what's already there.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(6);

/// Bumped by every answer request and by `cancel`; a stream whose id is no
/// longer current stops forwarding chunks, so pressing again or ending the
/// meeting supersedes an answer still being written.
static CURRENT: AtomicU64 = AtomicU64::new(0);

pub fn cancel() {
    CURRENT.fetch_add(1, Ordering::SeqCst);
}

fn is_current(id: u64) -> bool {
    CURRENT.load(Ordering::SeqCst) == id
}

#[derive(Clone, serde::Serialize)]
struct Start {
    id: u64,
}

#[derive(Clone, serde::Serialize)]
struct Chunk {
    id: u64,
    text: String,
}

#[derive(Clone, serde::Serialize)]
struct Done {
    id: u64,
    answer_text: String,
    /// Hotkey to first answer text on screen.
    first_token_ms: Option<u64>,
}

#[derive(Clone, serde::Serialize)]
struct Failure {
    id: u64,
    message: String,
}

fn fail(app: &AppHandle, id: u64, message: impl Into<String>) {
    if is_current(id) {
        let _ = app.emit("meeting-answer-error", Failure { id, message: message.into() });
    }
}

pub fn answer_now(app: &AppHandle) {
    let id = CURRENT.fetch_add(1, Ordering::SeqCst) + 1;
    super::reveal_panel(app);
    let _ = app.emit("meeting-answer-start", Start { id });

    let Some((ctrl, transcript)) = super::session_handles() else {
        let message = match super::toggle_key() {
            Some(key) => format!("Meeting mode isn't running. Press {} to start it.", key),
            None => "Meeting mode isn't running. Start it from the tray menu.".to_string(),
        };
        fail(app, id, &message);
        return;
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let pressed = Instant::now();

        // Get the phrase that was still being spoken into the transcript.
        let flushed = tauri::async_runtime::spawn_blocking(move || {
            let (tx, rx) = mpsc::channel();
            ctrl.send(Ctrl::Flush(tx)).is_ok() && rx.recv_timeout(FLUSH_TIMEOUT).is_ok()
        })
        .await
        .unwrap_or(false);
        let flush_ms = pressed.elapsed().as_millis();
        if !is_current(id) {
            return;
        }

        let lines = transcript.lock().unwrap_or_else(|e| e.into_inner()).recent(RECENT_SECS, RECENT_MAX_CHARS);
        if lines.is_empty() {
            fail(
                &app,
                id,
                "Nothing heard from the meeting yet. Meeting mode listens to everything this computer plays, on every speaker and headset, so check the call's audio is actually playing (not muted).",
            );
            return;
        }
        if cfg!(debug_assertions) {
            eprintln!(
                "[meeting] answer {}: {} line(s), flush {} in {} ms",
                id,
                lines.len(),
                if flushed { "done" } else { "timed out" },
                flush_ms
            );
        }

        if let Err(e) = stream_answer(&app, id, &lines, pressed).await {
            fail(&app, id, e);
        }
    });
}

fn payload(app: &AppHandle, lines: &[Line]) -> serde_json::Value {
    let transcript: Vec<_> = lines
        .iter()
        .map(|l| serde_json::json!({ "t": (l.t * 10.0).round() / 10.0, "text": l.text }))
        .collect();
    let mut body = serde_json::json!({
        "transcript": transcript,
        "brief": crate::commands::settings::meeting_brief(app),
        "documents": super::context::for_request(app),
        "gemini_api_key": crate::commands::settings::get_gemini_key_for_request(app.clone()).unwrap_or_default(),
    });
    if let (Some(target), serde_json::Value::Object(fields)) =
        (body.as_object_mut(), crate::commands::settings::llm_request_fields(app))
    {
        target.extend(fields);
    }
    body
}

async fn stream_answer(app: &AppHandle, id: u64, lines: &[Line], pressed: Instant) -> Result<(), String> {
    let url = format!("{}/api/meeting/answer-stream", API_BASE_URL);
    let res = reqwest::Client::new()
        .post(&url)
        .header("X-Pointr-Client-Key", CLIENT_KEY)
        .json(&payload(app, lines))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the Pointr backend: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Backend error ({}): {}", status, body));
    }

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    let mut first_token_ms: Option<u64> = None;

    while let Some(chunk) = stream.next().await {
        if !is_current(id) {
            return Ok(()); // superseded; dropping the stream closes it
        }
        let bytes = chunk.map_err(|e| format!("The answer stream broke off: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else { continue };
            match event.get("type").and_then(|v| v.as_str()) {
                Some("chunk") => {
                    if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                        if first_token_ms.is_none() {
                            first_token_ms = Some(pressed.elapsed().as_millis() as u64);
                        }
                        let _ = app.emit("meeting-answer-chunk", Chunk { id, text: text.to_string() });
                    }
                }
                Some("done") => {
                    let answer_text = event.get("answer_text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if cfg!(debug_assertions) {
                        eprintln!(
                            "[meeting] answer {}: first token {:?} ms, done {} ms",
                            id,
                            first_token_ms,
                            pressed.elapsed().as_millis()
                        );
                    }
                    let _ = app.emit("meeting-answer-done", Done { id, answer_text, first_token_ms });
                }
                _ => {}
            }
        }
    }
    Ok(())
}
