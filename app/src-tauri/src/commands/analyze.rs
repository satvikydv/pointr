use tauri::State;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::CaptureState;
use base64::Engine;
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

fn parse_analyze_response(json_res: &serde_json::Value) -> AnalyzeResponse {
    let answer = json_res.get("answer_text")
        .and_then(|v| v.as_str())
        .unwrap_or("No answer provided")
        .to_string();
    let pointer_target = json_res.get("pointer_target")
        .cloned()
        .filter(|v| !v.is_null());

    AnalyzeResponse { answer_text: answer, pointer_target }
}

#[tauri::command]
pub async fn process_crop(
    rect: Rect,
    query: Option<String>,
    state: State<'_, Mutex<CaptureState>>,
) -> Result<AnalyzeResponse, String> {
    let (monitor, image_base64) = {
        let state_lock = state.lock().unwrap();
        if state_lock.image_bytes.is_empty() {
            return Err("No image captured".into());
        }
        let monitor = state_lock.monitor.clone().unwrap();
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&state_lock.image_bytes);
        (monitor, image_base64)
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
    let session_id = Uuid::new_v4().to_string();
    
    // ISO8601 timestamp
    let timestamp = chrono::Utc::now().to_rfc3339();

    let client = reqwest::Client::new();
    
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
        "active_window_title": "Unknown", // Can be implemented later
        "query_text": query_text,
        "session_id": session_id,
        "timestamp": timestamp
    });

    let res = client.post("http://localhost:8000/api/analyze-screen")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !res.status().is_success() {
        let err_text = res.text().await.unwrap_or_default();
        return Err(format!("Backend error: {}", err_text));
    }

    let json_res: serde_json::Value = res.json().await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(parse_analyze_response(&json_res))
}

/// Primary hotkey path: no rect, no crop. Sends the full monitor capture
/// (already burned-in with a marker at the actual cursor position) straight
/// to the backend using the cursor position recorded at capture time.
#[tauri::command]
pub async fn process_direct(
    state: State<'_, Mutex<CaptureState>>,
) -> Result<AnalyzeResponse, String> {
    let (monitor, image_base64, cursor_norm) = {
        let state_lock = state.lock().unwrap();
        if state_lock.image_bytes.is_empty() {
            return Err("No image captured".into());
        }
        let monitor = state_lock.monitor.clone().unwrap();
        let cursor_norm = state_lock.cursor_norm
            .ok_or_else(|| "No cursor position captured".to_string())?;
        let image_base64 = base64::engine::general_purpose::STANDARD.encode(&state_lock.image_bytes);
        (monitor, image_base64, cursor_norm)
    };

    let session_id = Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().to_rfc3339();

    let client = reqwest::Client::new();

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
        "active_window_title": "Unknown", // Can be implemented later
        "query_text": "What is this?",
        "session_id": session_id,
        "timestamp": timestamp
    });

    let res = client.post("http://localhost:8000/api/analyze-screen")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    if !res.status().is_success() {
        let err_text = res.text().await.unwrap_or_default();
        return Err(format!("Backend error: {}", err_text));
    }

    let json_res: serde_json::Value = res.json().await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    Ok(parse_analyze_response(&json_res))
}
