use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Deliberately no screenshot field — the in-memory trace (main.js's
/// agentTraceLog) keeps those for live devtools debugging of the current
/// session, but persisting them to disk across every run would make this
/// file grow unbounded fast. History only needs to answer "what did it
/// decide and did it work", not "show me the pixels".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTraceStep {
    pub index: u32,
    pub decided_at: String,
    pub action_type: String,
    pub description: Option<String>,
    pub point: Option<[f32; 2]>,
    pub text: Option<String>,
    pub app_name: Option<String>,
    pub key: Option<String>,
    pub button: Option<String>,
    pub double: Option<bool>,
    pub direction: Option<String>,
    pub amount: Option<i32>,
    pub wait_ms: Option<i32>,
    pub answer_text: Option<String>,
    pub executed: bool,
    pub execution_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTraceRecord {
    pub id: String,
    pub task_description: String,
    pub plan: Vec<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub final_answer: Option<String>,
    pub stop_reason: Option<String>,
    pub steps: Vec<AgentTraceStep>,
}

/// Capped so the file stays small and reads stay cheap — with no
/// screenshots, even 50 full runs is a trivially small JSON file.
const MAX_HISTORY_RECORDS: usize = 50;

fn history_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to resolve config dir: {}", e))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    Ok(dir.join("agent_history.json"))
}

fn load_history(app: &AppHandle) -> Vec<AgentTraceRecord> {
    history_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_history(app: &AppHandle, records: &[AgentTraceRecord]) -> Result<(), String> {
    let path = history_path(app)?;
    let json = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| format!("Failed to write agent history: {}", e))
}

/// Appends one finished run. Called once per multi-step run, right after
/// the client-side trace is finalized (main.js's finishAgentTrace) —
/// regardless of how it ended (done/max_steps/aborted/error), since a
/// failed or cancelled run is exactly the kind of thing worth being able
/// to look back at.
#[tauri::command]
pub fn save_agent_trace(app: AppHandle, record: AgentTraceRecord) -> Result<(), String> {
    let mut records = load_history(&app);
    records.push(record);
    if records.len() > MAX_HISTORY_RECORDS {
        let excess = records.len() - MAX_HISTORY_RECORDS;
        records.drain(0..excess);
    }
    save_history(&app, &records)
}

/// Most recent first — the natural order for a history list UI.
#[tauri::command]
pub fn get_agent_history(app: AppHandle) -> Result<Vec<AgentTraceRecord>, String> {
    let mut records = load_history(&app);
    records.reverse();
    Ok(records)
}

#[tauri::command]
pub fn clear_agent_history(app: AppHandle) -> Result<(), String> {
    save_history(&app, &[])
}
