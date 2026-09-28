//! Uploads the screenshot while the user is still typing or speaking.
//!
//! The hotkey captures the screen immediately, but the question comes
//! seconds later. Starting the 1-3 MB upload at capture time means that by
//! the time the question is sent, the screenshot is usually already on the
//! server and the request only needs to carry a short reference.
//!
//! Every capture gets a sequence number, so an upload that finishes late
//! can never be attached to a newer capture. If the upload hasn't finished,
//! failed, or the server has since dropped it (older backend, expiry,
//! restart), requests fall back to sending the screenshot inline, which is
//! exactly the behaviour before this existed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use tauri::{AppHandle, Manager};

use crate::api_config::{API_BASE_URL, CLIENT_KEY};

/// The server keeps a staged screenshot for 60 s; stop using a ref well
/// before that so it can't expire between the check and the request.
const USABLE_FOR: Duration = Duration::from_secs(45);

/// How long a request waits for an upload that's already in flight before
/// giving up and sending inline. The upload started seconds earlier, so
/// what's left is usually much shorter than a fresh inline upload.
const WAIT_FOR_UPLOAD: Duration = Duration::from_secs(3);

enum Status {
    Idle,
    Uploading,
    Ready { reference: String, at: Instant },
    Failed,
}

pub struct StageState {
    seq: u64,
    status: Status,
}

impl StageState {
    pub fn new() -> Self {
        Self { seq: 0, status: Status::Idle }
    }
}

/// Starts uploading `png` for the capture just taken, replacing whatever
/// was staged for the previous one.
pub fn begin(app: &AppHandle, png: Vec<u8>) {
    let seq = {
        let state = app.state::<Mutex<StageState>>();
        let mut s = state.lock().unwrap();
        s.seq += 1;
        s.status = Status::Uploading;
        s.seq
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = upload(&png).await;
        let state = app.state::<Mutex<StageState>>();
        let mut s = state.lock().unwrap();
        if s.seq != seq {
            return; // a newer capture superseded this one
        }
        s.status = match result {
            Ok(reference) => Status::Ready { reference, at: Instant::now() },
            Err(e) => {
                if cfg!(debug_assertions) {
                    eprintln!("[stage] upload failed, will send inline: {}", e);
                }
                Status::Failed
            }
        };
    });
}

async fn upload(png: &[u8]) -> Result<String, String> {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let res = reqwest::Client::new()
        .post(format!("{}/api/stage-screenshot", API_BASE_URL))
        .header("X-Pointr-Client-Key", CLIENT_KEY)
        .timeout(Duration::from_secs(30))
        .json(&serde_json::json!({ "screenshot_base64": b64 }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("HTTP {}", res.status()));
    }
    let body: serde_json::Value = res.json().await.map_err(|e| e.to_string())?;
    body["ref"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "no ref in response".to_string())
}

/// The staged ref for the current capture, waiting briefly if its upload
/// is still in flight. None means "send the screenshot inline".
pub async fn current_ref(app: &AppHandle) -> Option<String> {
    let deadline = Instant::now() + WAIT_FOR_UPLOAD;
    loop {
        {
            let state = app.state::<Mutex<StageState>>();
            let s = state.lock().unwrap();
            match &s.status {
                Status::Ready { reference, at } if at.elapsed() < USABLE_FOR => return Some(reference.clone()),
                Status::Uploading if Instant::now() < deadline => {}
                _ => return None,
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// For the agent flow, which builds its request in JS: the ref if one is
/// ready, otherwise the screenshot inline. Exactly one of the two is set.
#[tauri::command]
pub async fn get_screenshot_for_request(
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    if let Some(reference) = current_ref(&app).await {
        return Ok(serde_json::json!({ "screenshot_base64": "", "screenshot_ref": reference }));
    }
    let b64 = crate::commands::analyze::get_current_screenshot_base64(app.state())?;
    Ok(serde_json::json!({ "screenshot_base64": b64, "screenshot_ref": "" }))
}

/// Posts `payload`, preferring the staged screenshot. If the server no
/// longer has it (HTTP 409), resends once with the screenshot inline.
pub async fn post_with_staged_screenshot(
    app: &AppHandle,
    url: &str,
    mut payload: serde_json::Value,
) -> Result<reqwest::Response, String> {
    let client = reqwest::Client::new();
    if let Some(reference) = current_ref(app).await {
        // Empty string, not null: the backend field is a plain str.
        let inline = std::mem::replace(&mut payload["screenshot_base64"], serde_json::Value::String(String::new()));
        payload["screenshot_ref"] = serde_json::Value::String(reference);
        let res = client
            .post(url)
            .header("X-Pointr-Client-Key", CLIENT_KEY)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))?;
        if res.status() != reqwest::StatusCode::CONFLICT {
            return Ok(res);
        }
        payload["screenshot_base64"] = inline;
        payload["screenshot_ref"] = serde_json::Value::String(String::new());
    }
    client
        .post(url)
        .header("X-Pointr-Client-Key", CLIENT_KEY)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))
}
