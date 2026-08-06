use tauri::{AppHandle, Emitter, State};
use serde::{Deserialize, Serialize};
use crate::CaptureState;
use base64::Engine;
use futures_util::StreamExt;
use std::sync::Mutex;

#[derive(Debug, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Serialize)]
pub struct AnalyzeResponse {
    pub answer_text: String,
    pub pointer_target: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoryboardStep {
    pub narration: String,
    pub shape: Option<String>,
    pub x_norm: Option<f32>,
    pub y_norm: Option<f32>,
    pub x2_norm: Option<f32>,
    pub y2_norm: Option<f32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct StoryboardResponse {
    pub steps: Vec<StoryboardStep>,
}

/// The `agent:` flow POSTs straight from JS via `fetch`, not through a Rust
/// command like process_direct/process_crop, so it has no other way to reach
/// the current capture — this hands it over as base64 the same way those
/// commands already encode it inline. Empty string (not an error) if nothing's
/// been captured yet, since not every agent task needs to see the screen.
#[tauri::command]
pub fn get_current_screenshot_base64(state: State<'_, Mutex<CaptureState>>) -> Result<String, String> {
    let state_lock = state.lock().unwrap();
    if state_lock.image_bytes.is_empty() {
        return Ok(String::new());
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(&state_lock.image_bytes))
}

/// POSTs to the backend's streaming endpoint and forwards each answer chunk
/// to the frontend as it arrives (event `analyze-stream-chunk`, tagged with
/// `request_id` so a stale/superseded request's chunks can be told apart
/// from the current one). Resolves once the stream's final "done" event
/// arrives, with the full answer + optional pointer target.
async fn post_and_stream(
    app: &AppHandle,
    request_id: &str,
    payload: serde_json::Value,
) -> Result<AnalyzeResponse, String> {
    println!("[{}] POST /api/analyze-screen-stream", request_id);
    let client = reqwest::Client::new();

    let res = client
        .post("http://localhost:8000/api/analyze-screen-stream")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !res.status().is_success() {
        let err_text = res.text().await.unwrap_or_default();
        return Err(format!("Backend error: {}", err_text));
    }

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();
    let mut final_answer = String::new();
    let mut final_pointer: Option<serde_json::Value> = None;
    let mut chunk_count = 0u32;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read failed: {}", e))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));

        while let Some(pos) = buffer.find('\n') {
            let line: String = buffer.drain(..=pos).collect();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            let event: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(_) => continue, // tolerate a malformed line rather than aborting the stream
            };

            match event.get("type").and_then(|v| v.as_str()) {
                Some("chunk") => {
                    if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                        chunk_count += 1;
                        let emit_result = app.emit(
                            "analyze-stream-chunk",
                            serde_json::json!({ "request_id": request_id, "text": text }),
                        );
                        if let Err(e) = emit_result {
                            eprintln!("[{}] failed to emit analyze-stream-chunk: {}", request_id, e);
                        }
                    }
                }
                Some("done") => {
                    final_answer = event
                        .get("answer_text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    final_pointer = event.get("pointer_target").cloned().filter(|v| !v.is_null());
                    println!(
                        "[{}] done: {} chunk(s), answer_len={}, pointer={}",
                        request_id,
                        chunk_count,
                        final_answer.len(),
                        final_pointer.is_some()
                    );
                }
                _ => {}
            }
        }
    }

    println!("[{}] stream ended, returning to frontend", request_id);
    Ok(AnalyzeResponse {
        answer_text: final_answer,
        pointer_target: final_pointer,
    })
}

#[tauri::command]
pub async fn process_crop(
    app: AppHandle,
    rect: Rect,
    query: Option<String>,
    request_id: String,
    state: State<'_, Mutex<CaptureState>>,
) -> Result<AnalyzeResponse, String> {
    let (monitor, image_base64, active_window_title, app_name, session_id, session_duration_secs) = {
        let state_lock = state.lock().unwrap();
        if state_lock.image_bytes.is_empty() {
            return Err("No image captured".into());
        }
        let monitor = state_lock.monitor.clone().unwrap();
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&state_lock.image_bytes);
        (
            monitor,
            image_base64,
            state_lock.active_window_title.clone(),
            state_lock.app_name.clone(),
            state_lock.session_id.clone(),
            state_lock.session_duration_secs,
        )
    };

    // Calculate normalized cursor from the center of the crop
    let center_x = rect.x + (rect.width / 2);
    let center_y = rect.y + (rect.height / 2);
    let coords = crate::capture::coords::normalize_cursor(
        monitor.origin_x + center_x,
        monitor.origin_y + center_y,
        &monitor,
    );

    let query_text = query.unwrap_or_else(|| "What is this?".to_string());
    let timestamp = chrono::Utc::now().to_rfc3339();

    let payload = serde_json::json!({
        "screenshot_base64": image_base64,
        "cursor_position": {
            "x_norm": coords.x_norm,
            "y_norm": coords.y_norm
        },
        "screen_resolution": {
            "width": monitor.width_px,
            "height": monitor.height_px
        },
        "active_window_title": active_window_title,
        "app_name": app_name,
        "session_duration_secs": session_duration_secs,
        "query_text": query_text,
        "session_id": session_id,
        "timestamp": timestamp
    });

    post_and_stream(&app, &request_id, payload).await
}

/// "explain: <topic>" mode — non-streaming, since the client needs the whole
/// ordered step list up front to play it back sequentially (marker + TTS per
/// step), not a growing block of text. Reuses the same capture state as the
/// primary direct-ask flow (current full-monitor screenshot, real cursor
/// position) — v1 is direct-hotkey only, not wired into the region-select
/// toolbar.
#[tauri::command]
pub async fn process_explain(
    topic: String,
    state: State<'_, Mutex<CaptureState>>,
) -> Result<StoryboardResponse, String> {
    let (monitor, image_base64, cursor_norm, active_window_title, app_name, session_id, session_duration_secs) = {
        let state_lock = state.lock().unwrap();
        if state_lock.image_bytes.is_empty() {
            return Err("No image captured".into());
        }
        let monitor = state_lock.monitor.clone().unwrap();
        let cursor_norm = state_lock
            .cursor_norm
            .ok_or_else(|| "No cursor position captured".to_string())?;
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&state_lock.image_bytes);
        (
            monitor,
            image_base64,
            cursor_norm,
            state_lock.active_window_title.clone(),
            state_lock.app_name.clone(),
            state_lock.session_id.clone(),
            state_lock.session_duration_secs,
        )
    };

    let timestamp = chrono::Utc::now().to_rfc3339();
    let payload = serde_json::json!({
        "screenshot_base64": image_base64,
        "cursor_position": {
            "x_norm": cursor_norm.0,
            "y_norm": cursor_norm.1
        },
        "screen_resolution": {
            "width": monitor.width_px,
            "height": monitor.height_px
        },
        "active_window_title": active_window_title,
        "app_name": app_name,
        "session_duration_secs": session_duration_secs,
        "query_text": topic,
        "session_id": session_id,
        "timestamp": timestamp
    });

    let client = reqwest::Client::new();
    let res = client
        .post("http://localhost:8000/api/analyze-explain")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !res.status().is_success() {
        let err_text = res.text().await.unwrap_or_default();
        return Err(format!("Backend error: {}", err_text));
    }

    res.json::<StoryboardResponse>()
        .await
        .map_err(|e| format!("Failed to parse storyboard response: {}", e))
}

/// Primary hotkey path: no rect, no crop. Sends the full monitor capture
/// (already burned-in with a marker at the actual cursor position) straight
/// to the backend using the cursor position recorded at capture time.
#[tauri::command]
pub async fn process_direct(
    app: AppHandle,
    query: Option<String>,
    request_id: String,
    state: State<'_, Mutex<CaptureState>>,
) -> Result<AnalyzeResponse, String> {
    let (monitor, image_base64, cursor_norm, active_window_title, app_name, session_id, session_duration_secs) = {
        let state_lock = state.lock().unwrap();
        if state_lock.image_bytes.is_empty() {
            return Err("No image captured".into());
        }
        let monitor = state_lock.monitor.clone().unwrap();
        let cursor_norm = state_lock
            .cursor_norm
            .ok_or_else(|| "No cursor position captured".to_string())?;
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&state_lock.image_bytes);
        (
            monitor,
            image_base64,
            cursor_norm,
            state_lock.active_window_title.clone(),
            state_lock.app_name.clone(),
            state_lock.session_id.clone(),
            state_lock.session_duration_secs,
        )
    };

    let query_text = query
        .filter(|q| !q.trim().is_empty())
        .unwrap_or_else(|| "What is this?".to_string());
    let timestamp = chrono::Utc::now().to_rfc3339();

    let payload = serde_json::json!({
        "screenshot_base64": image_base64,
        "cursor_position": {
            "x_norm": cursor_norm.0,
            "y_norm": cursor_norm.1
        },
        "screen_resolution": {
            "width": monitor.width_px,
            "height": monitor.height_px
        },
        "active_window_title": active_window_title,
        "app_name": app_name,
        "session_duration_secs": session_duration_secs,
        "query_text": query_text,
        "session_id": session_id,
        "timestamp": timestamp
    });

    post_and_stream(&app, &request_id, payload).await
}
