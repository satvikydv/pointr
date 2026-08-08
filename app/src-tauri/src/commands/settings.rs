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
    #[serde(default = "default_os_actions_enabled")]
    os_actions_enabled: bool,
    /// DPAPI-encrypted (`CryptProtectData`), then base64-encoded for JSON —
    /// tied to this Windows user account, so the ciphertext is useless
    /// copied to another machine or read by another user. Same underlying
    /// protection Windows Credential Manager itself is built on; storing it
    /// alongside the rest of settings.json (instead of a separate store)
    /// keeps this feature simple while still being real encryption, not
    /// plaintext-on-disk.
    #[serde(default)]
    github_token_encrypted: Option<String>,
}

fn default_speech_enabled() -> bool {
    true
}

fn default_os_actions_enabled() -> bool {
    true
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            voice_id: None,
            speech_enabled: true,
            os_actions_enabled: true,
            github_token_encrypted: None,
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

#[tauri::command]
pub fn get_os_actions_enabled(app: AppHandle) -> Result<bool, String> {
    Ok(load_settings(&app).os_actions_enabled)
}

#[tauri::command]
pub fn set_os_actions_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let mut settings = load_settings(&app);
    settings.os_actions_enabled = enabled;
    save_settings(&app, &settings)
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
