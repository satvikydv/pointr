use std::sync::Mutex;
use tauri::State;
use windows::core::HSTRING;
use windows::Media::Core::MediaSource;
use windows::Media::Playback::MediaPlayer;
use windows::Media::SpeechSynthesis::SpeechSynthesizer;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

/// Holds the currently-playing MediaPlayer so it isn't dropped (and playback
/// silently stopped) the instant `speak_text` returns — WinRT's MediaPlayer
/// only keeps playing while something keeps it alive.
pub struct TtsState {
    player: Mutex<Option<MediaPlayer>>,
}

impl TtsState {
    pub fn new() -> Self {
        Self {
            player: Mutex::new(None),
        }
    }
}

/// Picks the best available installed voice: a neural "Natural" voice if one
/// is installed (Windows 11 optional download, Settings > Time & Language >
/// Speech > Manage voices — far better quality than the legacy SAPI/OneCore
/// voices below), else the least-robotic of the legacy ones (Zira/David
/// tested better than the default Heera on this machine), else whatever the
/// system default is.
fn select_best_voice() -> Option<windows::Media::SpeechSynthesis::VoiceInformation> {
    let voices = SpeechSynthesizer::AllVoices().ok()?;
    let mut any_natural = None;
    let mut zira = None;

    for voice in &voices {
        let display = voice.DisplayName().unwrap_or_default().to_string();
        let is_natural = display.contains("Natural");
        // Known female neural voice names — return immediately, this is the
        // best case available.
        if is_natural && (display.contains("Aria") || display.contains("Jenny") || display.contains("Zira")) {
            return Some(voice);
        }
        if is_natural && any_natural.is_none() {
            any_natural = Some(voice.clone());
        }
        if zira.is_none() && display.contains("Zira") {
            zira = Some(voice.clone());
        }
    }

    any_natural.or(zira)
}

fn ensure_winrt_initialized() {
    unsafe {
        // Ignoring the result deliberately: Ok on first call per thread,
        // effectively a no-op "already initialized" signal on later calls
        // from a reused thread — both fine. A genuine apartment mismatch
        // would just surface as a clear error from the WinRT calls below.
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
}

#[tauri::command]
pub fn speak_text(text: String, state: State<'_, TtsState>) -> Result<(), String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(());
    }

    ensure_winrt_initialized();

    let synth = SpeechSynthesizer::new()
        .map_err(|e| format!("Failed to create speech synthesizer: {}", e))?;
    if let Some(voice) = select_best_voice() {
        let _ = synth.SetVoice(&voice);
    }
    // .get() blocks this (Tauri-managed, off the main/webview thread) thread
    // until synthesis finishes — same pattern as the WinRT OCR call, since
    // IAsyncOperation isn't directly awaitable here without extra glue.
    let stream = synth
        .SynthesizeTextToStreamAsync(&HSTRING::from(text))
        .map_err(|e| format!("Failed to start synthesis: {}", e))?
        .get()
        .map_err(|e| format!("Speech synthesis failed: {}", e))?;

    let content_type = stream
        .ContentType()
        .map_err(|e| format!("Failed to read synthesized stream content type: {}", e))?;
    let source = MediaSource::CreateFromStream(&stream, &content_type)
        .map_err(|e| format!("Failed to create media source: {}", e))?;

    let player =
        MediaPlayer::new().map_err(|e| format!("Failed to create media player: {}", e))?;
    player
        .SetSource(&source)
        .map_err(|e| format!("Failed to set media source: {}", e))?;
    player.Play().map_err(|e| format!("Failed to start playback: {}", e))?;

    *state.player.lock().unwrap() = Some(player);
    Ok(())
}

/// Cuts off any in-progress narration — called on dismiss so the voice
/// doesn't keep talking after the user has already closed the overlay.
#[tauri::command]
pub fn stop_speech(state: State<'_, TtsState>) -> Result<(), String> {
    if let Some(player) = state.player.lock().unwrap().take() {
        let _ = player.Pause();
    }
    Ok(())
}
