use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use windows::Media::SpeechSynthesis::SpeechSynthesizer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceOption {
    pub id: String,
    pub display_name: String,
    pub language: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedSettings {
    voice_id: Option<String>,
    #[serde(default = "default_speech_enabled")]
    speech_enabled: bool,
}

fn default_speech_enabled() -> bool {
    true
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            voice_id: None,
            speech_enabled: true,
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Failed to resolve config dir: {}", e))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    Ok(dir.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> PersistedSettings {
    settings_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: &PersistedSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| format!("Failed to write settings: {}", e))
}

#[tauri::command]
pub fn list_voices() -> Result<Vec<VoiceOption>, String> {
    let voices = SpeechSynthesizer::AllVoices()
        .map_err(|e| format!("Failed to enumerate voices: {}", e))?;
    Ok(voices
        .into_iter()
        .filter_map(|v| {
            Some(VoiceOption {
                id: v.Id().ok()?.to_string(),
                display_name: v.DisplayName().ok()?.to_string(),
                language: v.Language().ok()?.to_string(),
            })
        })
        .collect())
}

#[tauri::command]
pub fn get_selected_voice(app: AppHandle) -> Result<Option<String>, String> {
    Ok(load_settings(&app).voice_id)
}

#[tauri::command]
pub fn set_selected_voice(app: AppHandle, voice_id: String) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.voice_id = Some(voice_id);
    save_settings(&app, &settings)
}

#[tauri::command]
pub fn get_speech_enabled(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).speech_enabled)
}

#[tauri::command]
pub fn set_speech_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.speech_enabled = enabled;
    save_settings(&app, &settings)
}
