use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};
use windows::Media::SpeechSynthesis::SpeechSynthesizer;
use windows::Win32::Foundation::{HLOCAL, LocalFree};
use windows::Win32::Security::Cryptography::{CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB};

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
    /// Superseded by `action_permission`, kept only so an existing
    /// settings.json with actions switched off still reads as "off".
    #[serde(default = "default_os_actions_enabled")]
    os_actions_enabled: bool,
    /// "off" | "ask" | "allow". None means not chosen yet: derived from the
    /// legacy toggle (off stays off, otherwise "ask").
    #[serde(default)]
    action_permission: Option<String>,
    /// DPAPI-encrypted (`CryptProtectData`), then base64-encoded for JSON —
    /// tied to this Windows user account, so the ciphertext is useless
    /// copied to another machine or read by another user. Same underlying
    /// protection Windows Credential Manager itself is built on; storing it
    /// alongside the rest of settings.json (instead of a separate store)
    /// keeps this feature simple while still being real encryption, not
    /// plaintext-on-disk.
    #[serde(default)]
    github_token_encrypted: Option<String>,
    /// Same DPAPI treatment as github_token_encrypted — the user's own
    /// Gemini/Tavily API keys (BYOK), sent with every backend request
    /// instead of the server footing everyone's API bill on its own key.
    #[serde(default)]
    gemini_api_key_encrypted: Option<String>,
    #[serde(default)]
    tavily_api_key_encrypted: Option<String>,
    /// OpenAI is BYOK only (the server has no OpenAI key of its own), so
    /// choosing "openai" as the provider without this set fails fast with
    /// a clear message from the backend.
    #[serde(default)]
    openai_api_key_encrypted: Option<String>,
    /// "gemini" | "openai". Anything else is read as gemini.
    #[serde(default)]
    llm_provider: Option<String>,
    /// Model chosen per provider, so switching provider back and forth
    /// keeps each one's pick. None means that provider's DEFAULT_MODELS.
    #[serde(default)]
    gemini_model: Option<String>,
    #[serde(default)]
    openai_model: Option<String>,
    /// Push-to-talk (hold Ctrl+Win). On by default; it does nothing until
    /// the voice model is downloaded, and says so when first tried.
    #[serde(default = "default_voice_enabled")]
    voice_enabled: bool,
    /// Opt-in, default OFF — see commands/telemetry.rs. Anything that
    /// can't read this setting treats it as disabled (fail closed), so a
    /// corrupt/unreadable settings file never silently starts sending.
    #[serde(default)]
    telemetry_enabled: bool,
    /// Whether the one-time "share anonymous usage data?" prompt has been
    /// answered. Separate from the setting itself so that answering "Skip"
    /// is remembered and never asked again.
    #[serde(default)]
    telemetry_prompt_shown: bool,
    /// Random UUID generated on first use, persisted here. Deliberately
    /// NOT derived from machine id, username, MAC, or anything else
    /// identifying — it exists only so repeat events from one install can
    /// be grouped, and clearing settings.json resets it entirely.
    #[serde(default)]
    install_id: Option<String>,
}

fn default_speech_enabled() -> bool {
    true
}

fn default_os_actions_enabled() -> bool {
    true
}

fn default_voice_enabled() -> bool {
    true
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            voice_id: None,
            speech_enabled: true,
            os_actions_enabled: true,
            action_permission: None,
            github_token_encrypted: None,
            gemini_api_key_encrypted: None,
            tavily_api_key_encrypted: None,
            openai_api_key_encrypted: None,
            llm_provider: None,
            gemini_model: None,
            openai_model: None,
            voice_enabled: true,
            telemetry_enabled: false,
            telemetry_prompt_shown: false,
            install_id: None,
        }
    }
}

/// Encrypts `plaintext` with DPAPI, scoped to the current Windows user (no
/// explicit entropy/password — same default Credential Manager itself
/// uses). The output buffer is allocated by the OS (`LocalAlloc` under the
/// hood) and must be freed with `LocalFree`, not just dropped.
fn dpapi_protect(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&input, windows::core::PCWSTR::null(), None, None, None, 0, &mut output)
            .map_err(|e| format!("DPAPI encrypt failed: {}", e))?;

        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(bytes)
    }
}

/// Reverses `dpapi_protect`. Fails if the ciphertext was encrypted under a
/// different Windows user account (by design — that's the whole point).
fn dpapi_unprotect(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: ciphertext.len() as u32,
            pbData: ciphertext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
            .map_err(|e| format!("DPAPI decrypt failed: {}", e))?;

        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(bytes)
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

pub const ACTION_PERMISSIONS: [&str; 3] = ["off", "ask", "allow"];

fn resolve_action_permission(s: &PersistedSettings) -> &'static str {
    match s.action_permission.as_deref() {
        Some("off") => "off",
        Some("allow") => "allow",
        Some("ask") => "ask",
        // Unset or unrecognized: honour the legacy toggle, else the safe default.
        _ if !s.os_actions_enabled => "off",
        _ => "ask",
    }
}

pub(crate) fn voice_enabled(app: &AppHandle) -> bool {
    load_settings(app).voice_enabled
}

/// Persists the toggle only; the command (voice::set_voice_enabled) also
/// loads or frees the model.
pub(crate) fn persist_voice_enabled(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = load_settings(app);
    settings.voice_enabled = enabled;
    save_settings(app, &settings)
}

pub(crate) fn action_permission(app: &AppHandle) -> &'static str {
    resolve_action_permission(&load_settings(app))
}

/// Whether Pointr may act on screen, and whether it asks first.
#[tauri::command]
pub fn get_action_permission(app: AppHandle) -> Result<String, String> {
    Ok(resolve_action_permission(&load_settings(&app)).to_string())
}

#[tauri::command]
pub fn set_action_permission(app: AppHandle, permission: String) -> Result<(), String> {
    if !ACTION_PERMISSIONS.contains(&permission.as_str()) {
        return Err(format!("Unknown action permission: {}", permission));
    }
    let mut settings = load_settings(&app);
    // Kept in step so an older build reading this file still honours "off".
    settings.os_actions_enabled = permission != "off";
    settings.action_permission = Some(permission);
    save_settings(&app, &settings)
}

#[cfg(test)]
mod action_permission_tests {
    use super::*;

    #[test]
    fn legacy_toggle_maps_to_off_or_ask() {
        let mut s = PersistedSettings::default();
        assert_eq!(resolve_action_permission(&s), "ask");
        s.os_actions_enabled = false;
        assert_eq!(resolve_action_permission(&s), "off");
    }

    #[test]
    fn explicit_choice_wins_over_legacy_toggle() {
        let mut s = PersistedSettings::default();
        s.os_actions_enabled = false;
        s.action_permission = Some("allow".into());
        assert_eq!(resolve_action_permission(&s), "allow");
    }

    #[test]
    fn unknown_value_falls_back_safely() {
        let mut s = PersistedSettings::default();
        s.action_permission = Some("yolo".into());
        assert_eq!(resolve_action_permission(&s), "ask");
    }
}

#[tauri::command]
pub fn save_github_token(app: AppHandle, token: String) -> Result<(), String> {
    let encrypted = dpapi_protect(token.as_bytes())?;
    let mut settings = load_settings(&app);
    settings.github_token_encrypted = Some(general_purpose::STANDARD.encode(encrypted));
    save_settings(&app, &settings)
}

/// Whether a token is stored — never the token itself. The Settings UI only
/// ever needs to know connected/not-connected, so there's no path for the
/// raw secret to reach the frontend except `get_github_token_for_request`,
/// which is called only when an agent task actually needs it.
#[tauri::command]
pub fn get_github_token_status(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).github_token_encrypted.is_some())
}

#[tauri::command]
pub fn clear_github_token(app: AppHandle) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.github_token_encrypted = None;
    save_settings(&app, &settings)
}

/// Decrypts and returns the real token — used only on the request path
/// (threaded through to the backend the same way clipboard_text/
/// screenshot_base64 already are), never for display. Empty string if
/// nothing's connected, not an error, matching `get_current_screenshot_base64`'s
/// "no capture yet" convention.
#[tauri::command]
pub fn get_github_token_for_request(app: AppHandle) -> Result<String, String> {
    let settings = load_settings(&app);
    let Some(encoded) = settings.github_token_encrypted else {
        return Ok(String::new());
    };
    let encrypted = general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|e| format!("Corrupt stored token: {}", e))?;
    let plaintext = dpapi_unprotect(&encrypted)?;
    String::from_utf8(plaintext).map_err(|e| format!("Corrupt stored token: {}", e))
}

#[tauri::command]
pub fn save_gemini_key(app: AppHandle, key: String) -> Result<(), String> {
    let encrypted = dpapi_protect(key.as_bytes())?;
    let mut settings = load_settings(&app);
    settings.gemini_api_key_encrypted = Some(general_purpose::STANDARD.encode(encrypted));
    save_settings(&app, &settings)
}

#[tauri::command]
pub fn get_gemini_key_status(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).gemini_api_key_encrypted.is_some())
}

#[tauri::command]
pub fn clear_gemini_key(app: AppHandle) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.gemini_api_key_encrypted = None;
    save_settings(&app, &settings)
}

#[tauri::command]
pub fn get_gemini_key_for_request(app: AppHandle) -> Result<String, String> {
    let settings = load_settings(&app);
    let Some(encoded) = settings.gemini_api_key_encrypted else {
        return Ok(String::new());
    };
    let encrypted = general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|e| format!("Corrupt stored key: {}", e))?;
    let plaintext = dpapi_unprotect(&encrypted)?;
    String::from_utf8(plaintext).map_err(|e| format!("Corrupt stored key: {}", e))
}

#[tauri::command]
pub fn save_tavily_key(app: AppHandle, key: String) -> Result<(), String> {
    let encrypted = dpapi_protect(key.as_bytes())?;
    let mut settings = load_settings(&app);
    settings.tavily_api_key_encrypted = Some(general_purpose::STANDARD.encode(encrypted));
    save_settings(&app, &settings)
}

#[tauri::command]
pub fn get_tavily_key_status(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).tavily_api_key_encrypted.is_some())
}

#[tauri::command]
pub fn clear_tavily_key(app: AppHandle) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.tavily_api_key_encrypted = None;
    save_settings(&app, &settings)
}

#[tauri::command]
pub fn get_tavily_key_for_request(app: AppHandle) -> Result<String, String> {
    let settings = load_settings(&app);
    let Some(encoded) = settings.tavily_api_key_encrypted else {
        return Ok(String::new());
    };
    let encrypted = general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|e| format!("Corrupt stored key: {}", e))?;
    let plaintext = dpapi_unprotect(&encrypted)?;
    String::from_utf8(plaintext).map_err(|e| format!("Corrupt stored key: {}", e))
}

#[tauri::command]
pub fn save_openai_key(app: AppHandle, key: String) -> Result<(), String> {
    let encrypted = dpapi_protect(key.as_bytes())?;
    let mut settings = load_settings(&app);
    settings.openai_api_key_encrypted = Some(general_purpose::STANDARD.encode(encrypted));
    save_settings(&app, &settings)
}

#[tauri::command]
pub fn get_openai_key_status(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).openai_api_key_encrypted.is_some())
}

#[tauri::command]
pub fn clear_openai_key(app: AppHandle) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.openai_api_key_encrypted = None;
    save_settings(&app, &settings)
}

fn get_openai_key_for_request(app: &AppHandle) -> Result<String, String> {
    let settings = load_settings(app);
    let Some(encoded) = settings.openai_api_key_encrypted else {
        return Ok(String::new());
    };
    let encrypted = general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|e| format!("Corrupt stored key: {}", e))?;
    let plaintext = dpapi_unprotect(&encrypted)?;
    String::from_utf8(plaintext).map_err(|e| format!("Corrupt stored key: {}", e))
}

// ---------------------------------------------------------------------
// Model provider + model choice
// ---------------------------------------------------------------------

pub const PROVIDERS: [&str; 2] = ["gemini", "openai"];

/// Keep in sync with DEFAULT_MODELS in backend/app/services/llm.py. The
/// backend applies the same defaults to an empty model, so a mismatch only
/// changes what Settings displays, never which model answers.
pub fn default_model(provider: &str) -> &'static str {
    match provider {
        "openai" => "gpt-6-luna",
        _ => "gemini-3.1-flash-lite",
    }
}

/// Shown before a key is connected, or if the live list can't be fetched.
/// Live listing (`list_models`) replaces this with what the key can
/// actually use.
fn fallback_models(provider: &str) -> Vec<String> {
    let ids: &[&str] = match provider {
        "openai" => &["gpt-6-luna", "gpt-6-sol", "gpt-6-astra"],
        _ => &[
            "gemini-3.1-flash-lite",
            "gemini-3.5-flash-lite",
            "gemini-3.5-flash",
            "gemini-3.6-flash",
            "gemini-3.7-flash",
            "gemini-3.8-flash",
            "gemini-3.1-pro-preview",
        ],
    };
    ids.iter().map(|s| s.to_string()).collect()
}

fn normalize_provider(provider: Option<&str>) -> &'static str {
    match provider {
        Some("openai") => "openai",
        _ => "gemini",
    }
}

#[derive(Debug, Serialize)]
pub struct ModelSettings {
    pub provider: String,
    pub gemini_model: String,
    pub openai_model: String,
}

#[tauri::command]
pub fn get_model_settings(app: AppHandle) -> Result<ModelSettings, String> {
    let s = load_settings(&app);
    Ok(ModelSettings {
        provider: normalize_provider(s.llm_provider.as_deref()).to_string(),
        gemini_model: s.gemini_model.unwrap_or_else(|| default_model("gemini").to_string()),
        openai_model: s.openai_model.unwrap_or_else(|| default_model("openai").to_string()),
    })
}

#[tauri::command]
pub fn set_llm_provider(app: AppHandle, provider: String) -> Result<(), String> {
    if !PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("Unknown provider: {}", provider));
    }
    let mut settings = load_settings(&app);
    settings.llm_provider = Some(provider);
    save_settings(&app, &settings)
}

/// Any model id is accepted (the free-text box exists for ids not in the
/// list), trimmed; blank resets to the provider default.
#[tauri::command]
pub fn set_llm_model(app: AppHandle, provider: String, model: String) -> Result<(), String> {
    let model = model.trim();
    let value = if model.is_empty() { None } else { Some(model.to_string()) };
    let mut settings = load_settings(&app);
    match provider.as_str() {
        "gemini" => settings.gemini_model = value,
        "openai" => settings.openai_model = value,
        _ => return Err(format!("Unknown provider: {}", provider)),
    }
    save_settings(&app, &settings)
}

/// The provider/model fields every backend request carries. The OpenAI key
/// is included only when OpenAI is the chosen provider, so it never leaves
/// the machine for a request that won't use it.
fn chosen_provider_model(app: &AppHandle) -> (&'static str, String) {
    let s = load_settings(app);
    let provider = normalize_provider(s.llm_provider.as_deref());
    let model = match provider {
        "openai" => s.openai_model,
        _ => s.gemini_model,
    }
    .unwrap_or_else(|| default_model(provider).to_string());
    (provider, model)
}

pub(crate) fn llm_request_fields(app: &AppHandle) -> serde_json::Value {
    let (provider, model) = chosen_provider_model(app);
    let openai_api_key = if provider == "openai" {
        get_openai_key_for_request(app).unwrap_or_default()
    } else {
        String::new()
    };
    serde_json::json!({
        "provider": provider,
        "model": model,
        "openai_api_key": openai_api_key,
    })
}

/// Provider and model for telemetry. The model comes from a free-text box,
/// so anything that doesn't look like a real model id is reported as
/// "custom" rather than sent as typed.
pub(crate) fn telemetry_model_info(app: &AppHandle) -> (String, String) {
    let (provider, model) = chosen_provider_model(app);
    let looks_like_model_id = model.len() <= 64
        && (model.starts_with("gemini-") || model.starts_with("gpt-"))
        && model.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.');
    let model = if looks_like_model_id { model } else { "custom".to_string() };
    (provider.to_string(), model)
}

/// Same fields for main.js, which builds the agent request bodies itself.
#[tauri::command]
pub fn get_llm_request_fields(app: AppHandle) -> Result<serde_json::Value, String> {
    Ok(llm_request_fields(&app))
}

/// Non-chat model families a provider's list endpoint also returns. Pointr
/// sends a screenshot plus text and expects text back, so these can't work.
/// The last three are Gemini variants that need special tool setups.
const NON_CHAT_MARKERS: [&str; 16] = [
    "tts", "audio", "realtime", "transcribe", "translate", "live", "image",
    "embedding", "whisper", "dall-e", "moderation", "search", "instruct",
    "computer-use", "robotics", "customtools",
];

fn is_chat_model(id: &str) -> bool {
    !NON_CHAT_MARKERS.iter().any(|m| id.contains(m))
}

fn filter_gemini_models(body: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = body["models"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|m| {
            m["supportedGenerationMethods"]
                .as_array()
                .is_some_and(|methods| methods.iter().any(|x| x == "generateContent"))
        })
        .filter_map(|m| m["name"].as_str())
        .map(|name| name.trim_start_matches("models/").to_string())
        .filter(|id| id.starts_with("gemini-") && is_chat_model(id))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn filter_openai_models(body: &serde_json::Value) -> Vec<String> {
    let mut ids: Vec<String> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str())
        .filter(|id| id.starts_with("gpt-") && is_chat_model(id))
        .map(str::to_string)
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[derive(Debug, Serialize)]
pub struct ModelList {
    pub models: Vec<String>,
    /// true when fetched from the provider with the user's key; false when
    /// this is the built-in fallback list.
    pub live: bool,
}

/// Lists the models the user's own key can use, filtered to ones that take
/// an image and return text. Falls back to the built-in list with no key
/// (a Gemini user relying on the hosted key has nothing to list with) or
/// on any error, so the dropdown is never empty.
#[tauri::command]
pub async fn list_models(app: AppHandle, provider: String) -> Result<ModelList, String> {
    let provider = normalize_provider(Some(provider.as_str()));
    let fallback = || ModelList { models: fallback_models(provider), live: false };

    let key = match provider {
        "openai" => get_openai_key_for_request(&app).unwrap_or_default(),
        _ => get_gemini_key_for_request(app.clone()).unwrap_or_default(),
    };
    if key.is_empty() {
        return Ok(fallback());
    }

    let client = reqwest::Client::new();
    // Key goes in a header, never the URL, so it can't end up in a log line.
    let request = match provider {
        "openai" => client.get("https://api.openai.com/v1/models").bearer_auth(&key),
        _ => client
            .get("https://generativelanguage.googleapis.com/v1beta/models?pageSize=1000")
            .header("x-goog-api-key", &key),
    };
    let body: serde_json::Value = match request.timeout(std::time::Duration::from_secs(10)).send().await {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(v) => v,
            Err(_) => return Ok(fallback()),
        },
        _ => return Ok(fallback()),
    };

    let models = match provider {
        "openai" => filter_openai_models(&body),
        _ => filter_gemini_models(&body),
    };
    if models.is_empty() {
        return Ok(fallback());
    }
    Ok(ModelList { models, live: true })
}

#[cfg(test)]
mod model_tests {
    use super::*;

    #[test]
    fn defaults_match_the_backend() {
        assert_eq!(default_model("gemini"), "gemini-3.1-flash-lite");
        assert_eq!(default_model("openai"), "gpt-6-luna");
        assert_eq!(default_model("anything-else"), "gemini-3.1-flash-lite");
    }

    #[test]
    fn fallback_lists_start_with_the_default() {
        for p in PROVIDERS {
            assert_eq!(fallback_models(p)[0], default_model(p));
        }
    }

    #[test]
    fn gemini_filter_keeps_only_content_models() {
        let body = serde_json::json!({"models": [
            {"name": "models/gemini-3.5-flash", "supportedGenerationMethods": ["generateContent", "countTokens"]},
            {"name": "models/gemini-3.8-flash-tts", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/gemini-3.1-flash-image", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/gemini-3.8-live", "supportedGenerationMethods": ["bidiGenerateContent"]},
            {"name": "models/gemini-embedding-001", "supportedGenerationMethods": ["embedContent"]},
            {"name": "models/imagen-4", "supportedGenerationMethods": ["predict"]},
        ]});
        assert_eq!(filter_gemini_models(&body), vec!["gemini-3.5-flash"]);
    }

    #[test]
    fn openai_filter_keeps_only_chat_models() {
        let body = serde_json::json!({"data": [
            {"id": "gpt-6-sol"}, {"id": "gpt-6-luna"}, {"id": "gpt-realtime"},
            {"id": "gpt-4o-mini-tts"}, {"id": "gpt-image-1"}, {"id": "text-embedding-3-small"},
            {"id": "whisper-1"}, {"id": "gpt-4o-transcribe"},
        ]});
        assert_eq!(filter_openai_models(&body), vec!["gpt-6-luna", "gpt-6-sol"]);
    }
}

#[cfg(test)]
mod dpapi_tests {
    use super::*;

    #[test]
    fn round_trips_a_real_token_shaped_secret() {
        let secret = "ghp_1234567890abcdefABCDEFghijklmnopQRST";
        let encrypted = dpapi_protect(secret.as_bytes()).expect("encrypt failed");
        assert_ne!(encrypted, secret.as_bytes(), "ciphertext must not equal plaintext");
        let decrypted = dpapi_unprotect(&encrypted).expect("decrypt failed");
        assert_eq!(decrypted, secret.as_bytes());
    }

    #[test]
    fn tampered_ciphertext_fails_to_decrypt() {
        let mut encrypted = dpapi_protect(b"some-secret").expect("encrypt failed");
        let last = encrypted.len() - 1;
        encrypted[last] ^= 0xFF;
        assert!(dpapi_unprotect(&encrypted).is_err());
    }
}

// ---------------------------------------------------------------------
// Telemetry settings (see commands/telemetry.rs)
//
// Every read here fails CLOSED: any error reading settings.json is
// reported as "telemetry disabled", never as enabled.
// ---------------------------------------------------------------------

/// Internal, non-command read used by the capture path itself — a Tauri
/// command can only be called from the frontend, but telemetry fires from
/// Rust too.
pub(crate) fn telemetry_enabled(app: &AppHandle) -> bool {
    load_settings(app).telemetry_enabled
}

/// Returns the persisted anonymous install id, generating and saving one
/// on first call. `None` if it couldn't be persisted — the caller treats
/// that as "don't send", rather than inventing a throwaway id per event
/// (which would inflate user counts and still identify nothing useful).
pub(crate) fn install_id(app: &AppHandle) -> Option<String> {
    let mut settings = load_settings(app);
    if let Some(existing) = &settings.install_id {
        return Some(existing.clone());
    }
    let fresh = uuid::Uuid::new_v4().to_string();
    settings.install_id = Some(fresh.clone());
    save_settings(app, &settings).ok()?;
    Some(fresh)
}

#[tauri::command]
pub fn get_telemetry_enabled(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).telemetry_enabled)
}

#[tauri::command]
pub fn set_telemetry_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.telemetry_enabled = enabled;
    save_settings(&app, &settings)
}

/// True once the one-time opt-in prompt has been answered either way.
#[tauri::command]
pub fn get_telemetry_prompt_shown(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).telemetry_prompt_shown)
}

#[tauri::command]
pub fn set_telemetry_prompt_shown(app: AppHandle, shown: bool) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.telemetry_prompt_shown = shown;
    save_settings(&app, &settings)
}
