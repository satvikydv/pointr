//! Documents the user adds in Settings as background for meeting answers:
//! a resume, project write-ups, notes. Their text is the source of truth
//! about the user when an answer is written (see the backend prompt in
//! routes/meeting.py).
//!
//! Text is pulled out once, when a file is added: on this machine for text
//! and Markdown, by the backend for PDF and Word (`/api/meeting/extract-text`,
//! which keeps nothing). Only the text is stored, in the local app-data
//! folder (not the roaming profile), one `<id>.txt` per document plus an
//! `index.json`. It leaves the machine only as part of a meeting answer
//! request. Adding a file with the same name as one already there replaces
//! it, so re-adding an updated resume just works.

use std::path::PathBuf;
use std::sync::Mutex;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::doc_text::{self, Kind};
use crate::api_config::{API_BASE_URL, CLIENT_KEY};

/// Largest file accepted. Resumes and write-ups are far smaller; this stops
/// someone dropping in a 200 MB scan by mistake.
const MAX_FILE_BYTES: usize = 15 * 1024 * 1024;

/// Most documents kept (the backend accepts 20 per request).
const MAX_DOCS: usize = 20;

/// Serialises index read-modify-write between overlapping adds/removes.
static INDEX_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocInfo {
    pub id: String,
    pub name: String,
    pub chars: usize,
    pub words: usize,
    /// Longer than MAX_DOC_CHARS, so only the start was kept.
    pub truncated: bool,
    pub added_at: String,
}

#[derive(Serialize)]
pub struct ContextDocs {
    docs: Vec<DocInfo>,
    total_chars: usize,
    /// How much document text one answer can carry; past this, later
    /// documents are cut or left out (Settings says so).
    budget_chars: usize,
}

fn dir(app: &AppHandle) -> Result<PathBuf, String> {
    let base = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("Failed to resolve data dir: {}", e))?;
    let dir = base.join("meeting-context");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create the documents folder: {}", e))?;
    Ok(dir)
}

fn load_index(dir: &std::path::Path) -> Vec<DocInfo> {
    std::fs::read_to_string(dir.join("index.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_index(dir: &std::path::Path, docs: &[DocInfo]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(docs).map_err(|e| e.to_string())?;
    // Written whole then renamed, so a crash mid-write can't leave a
    // half-written index that loses every document.
    let tmp = dir.join("index.json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("Failed to save the documents list: {}", e))?;
    std::fs::rename(&tmp, dir.join("index.json")).map_err(|e| format!("Failed to save the documents list: {}", e))
}

/// Document ids are uuids we generated; anything else in the index is
/// ignored rather than used as a path.
fn text_path(dir: &std::path::Path, id: &str) -> Option<PathBuf> {
    uuid::Uuid::parse_str(id).ok().map(|u| dir.join(format!("{}.txt", u)))
}

fn summary(docs: Vec<DocInfo>) -> ContextDocs {
    let total_chars = docs.iter().map(|d| d.chars).sum();
    ContextDocs { docs, total_chars, budget_chars: doc_text::SEND_BUDGET_CHARS }
}

#[tauri::command]
pub fn list_context_docs(app: AppHandle) -> Result<ContextDocs, String> {
    let dir = dir(&app)?;
    Ok(summary(load_index(&dir)))
}

/// The backend's error for a failed extraction, as the user should see it.
async fn backend_error(res: reqwest::Response) -> String {
    let status = res.status();
    let body: serde_json::Value = res.json().await.unwrap_or_default();
    match body.get("detail").and_then(|d| d.as_str()) {
        Some(detail) => detail.to_string(),
        None => format!("The Pointr backend couldn't read this file (HTTP {}).", status.as_u16()),
    }
}

async fn extract_remotely(name: &str, data_base64: &str) -> Result<String, String> {
    let res = reqwest::Client::new()
        .post(format!("{}/api/meeting/extract-text", API_BASE_URL))
        .header("X-Pointr-Client-Key", CLIENT_KEY)
        .json(&serde_json::json!({ "filename": name, "data_base64": data_base64 }))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach the Pointr backend to read this file: {}", e))?;
    if !res.status().is_success() {
        return Err(backend_error(res).await);
    }
    let body: serde_json::Value = res.json().await.map_err(|e| format!("Unexpected reply from the backend: {}", e))?;
    Ok(body.get("text").and_then(|t| t.as_str()).unwrap_or_default().to_string())
}

/// Adds (or replaces, by file name) a document. `data_base64` is the
/// file's bytes as read by the Settings page.
#[tauri::command]
pub async fn add_context_doc(app: AppHandle, name: String, data_base64: String) -> Result<DocInfo, String> {
    // A display name only; never used as a path.
    let name: String = name.rsplit(['/', '\\']).next().unwrap_or("").trim().chars().take(120).collect();
    if name.is_empty() {
        return Err("The file has no name.".into());
    }
    let kind = doc_text::kind_of(&name)
        .ok_or_else(|| format!("{} isn't a supported file. Add a PDF, Word (.docx), text or Markdown file.", name))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|_| "The file couldn't be read.".to_string())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(format!("{} is larger than {} MB.", name, MAX_FILE_BYTES / (1024 * 1024)));
    }

    let raw = match kind {
        Kind::PlainText => doc_text::decode_text(&bytes),
        Kind::Extracted => extract_remotely(&name, &data_base64).await?,
    };
    let text = doc_text::tidy(&raw);
    if text.is_empty() {
        return Err(format!("No text found in {}.", name));
    }
    let (text, truncated) = doc_text::truncate_chars(&text, doc_text::MAX_DOC_CHARS);

    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = dir(&app)?;
    let mut docs = load_index(&dir);
    let existing = docs.iter().position(|d| d.name.eq_ignore_ascii_case(&name));
    if existing.is_none() && docs.len() >= MAX_DOCS {
        return Err(format!("You can add up to {} documents. Remove one first.", MAX_DOCS));
    }
    let id = existing
        .map(|i| docs[i].id.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let path = text_path(&dir, &id).ok_or("Corrupt documents list.")?;
    std::fs::write(&path, &text).map_err(|e| format!("Failed to save {}: {}", name, e))?;

    let info = DocInfo {
        id,
        name,
        chars: text.chars().count(),
        words: doc_text::word_count(&text),
        truncated,
        added_at: chrono::Utc::now().to_rfc3339(),
    };
    match existing {
        Some(i) => docs[i] = info.clone(),
        None => docs.push(info.clone()),
    }
    save_index(&dir, &docs)?;
    Ok(info)
}

#[tauri::command]
pub fn remove_context_doc(app: AppHandle, id: String) -> Result<(), String> {
    let _guard = INDEX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = dir(&app)?;
    let mut docs = load_index(&dir);
    docs.retain(|d| d.id != id);
    save_index(&dir, &docs)?;
    if let Some(path) = text_path(&dir, &id) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// The documents for one answer request, as `{name, text}` objects, in the
/// order they were added and cut to SEND_BUDGET_CHARS.
pub(crate) fn for_request(app: &AppHandle) -> Vec<serde_json::Value> {
    let Ok(dir) = dir(app) else { return Vec::new() };
    let docs: Vec<(String, String)> = load_index(&dir)
        .into_iter()
        .filter_map(|d| {
            let text = std::fs::read_to_string(text_path(&dir, &d.id)?).ok()?;
            Some((d.name, text))
        })
        .collect();
    doc_text::fit_budget(docs, doc_text::SEND_BUDGET_CHARS)
        .into_iter()
        .map(|(name, text)| serde_json::json!({ "name": name, "text": text }))
        .collect()
}
