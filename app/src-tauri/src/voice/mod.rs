//! Push-to-talk: hold Ctrl+Win, speak, release.
//!
//! Latency is the whole point, so work happens while the key is held
//! rather than after release:
//!
//! 1. Ctrl+Win goes down: the mic opens immediately (so the first word
//!    isn't lost to the engage delay below).
//! 2. Held for ENGAGE_DELAY with no other key: this is push-to-talk, not a
//!    Windows shortcut. The screen is captured (same pipeline as the typed
//!    hotkey) and the overlay shows a listening state.
//! 3. While held: every time the speaker pauses, the finished phrase is
//!    transcribed on-device and shown as a live partial transcript.
//! 4. Release: only the last unfinished phrase is left to transcribe. The
//!    final text goes to the frontend, which sends it through the exact
//!    same path as a typed question.
//!
//! Speech-to-text runs on this machine by default (Parakeet via ONNX
//! Runtime), so audio never leaves it. Settings can instead send it to
//! OpenAI under the user's own key (voice/remote.rs), phrase by phrase in
//! the same way.

pub mod audio;
pub mod hotkey;
pub mod model;
pub mod remote;
pub mod segment;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use transcribe_rs::onnx::parakeet::{ParakeetModel, ParakeetParams};
use transcribe_rs::onnx::Quantization;

use hotkey::HookEvent;

/// How long Ctrl+Win must be held, with nothing else pressed, before it
/// counts as push-to-talk. Short enough to feel instant, long enough that
/// Ctrl+Win+D / Ctrl+Win+Left (typed quickly) never trigger it.
const ENGAGE_DELAY: Duration = Duration::from_millis(250);

/// How often the transcriber checks for a finished phrase and reports the
/// mic level to the overlay.
const TICK: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------
// Speech engine
// ---------------------------------------------------------------------

/// Loaded once and kept for the life of the app (about 1 GB of RAM):
/// loading takes a few seconds, which would otherwise land on the first
/// press. Behind a mutex so a press during the background preload simply
/// waits for it instead of loading a second copy.
static ENGINE: Mutex<Option<ParakeetModel>> = Mutex::new(None);

fn with_engine<T>(app: &AppHandle, f: impl FnOnce(&mut ParakeetModel) -> T) -> Result<T, String> {
    let mut guard = ENGINE.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        let dir = model::model_dir(app)?;
        if !model::is_installed(&dir) {
            return Err("The voice model isn't downloaded yet.".into());
        }
        let started = Instant::now();
        let loaded = ParakeetModel::load(&dir, &Quantization::Int8)
            .map_err(|e| format!("Failed to load the voice model: {}", e))?;
        if cfg!(debug_assertions) {
            eprintln!("[voice] model loaded in {:?}", started.elapsed());
        }
        *guard = Some(loaded);
    }
    Ok(f(guard.as_mut().unwrap()))
}

/// Which speech-to-text engine a press uses, fixed when the press starts.
#[derive(Clone)]
enum Engine {
    Local,
    OpenAi { api_key: String },
}

impl Engine {
    fn name(&self) -> &'static str {
        match self {
            Engine::Local => "local",
            Engine::OpenAi { .. } => "openai",
        }
    }
}

fn transcribe(app: &AppHandle, engine: &Engine, samples: &[f32]) -> Result<String, String> {
    match engine {
        Engine::Local => with_engine(app, |m| m.transcribe_with(samples, &ParakeetParams::default()))?
            .map(|r| r.text.trim().to_string())
            .map_err(|e| format!("Transcription failed: {}", e)),
        Engine::OpenAi { api_key } => remote::transcribe(api_key, samples),
    }
}

/// Loads the model in the background if it's installed and voice is on,
/// so the first press is as fast as every other.
pub fn preload(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        if !crate::commands::settings::voice_enabled(&app)
            || crate::commands::settings::stt_engine(&app) != "local"
        {
            return;
        }
        let installed = model::model_dir(&app).map(|d| model::is_installed(&d)).unwrap_or(false);
        if installed {
            if let Err(e) = with_engine(&app, |_| ()) {
                eprintln!("[voice] preload failed: {}", e);
            }
        }
    });
}

// ---------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------

/// Installs the keyboard hook and starts the push-to-talk controller.
pub fn start(app: &AppHandle) {
    let (tx, rx) = mpsc::channel();
    hotkey::install(tx);
    let app_for_thread = app.clone();
    std::thread::Builder::new()
        .name("pointr-ptt".into())
        .spawn(move || controller(app_for_thread, rx))
        .expect("failed to spawn push-to-talk controller");
    preload(app);
}

#[derive(Clone, serde::Serialize)]
struct Message {
    message: String,
}

fn emit_error(app: &AppHandle, message: impl Into<String>) {
    let _ = app.emit("voice-error", Message { message: message.into() });
}

/// The engine for a new press, or why a press can't record right now.
/// Checked before the mic opens, so an unusable press never flashes the
/// Windows mic indicator.
fn resolve_engine(app: &AppHandle) -> Result<Engine, String> {
    if crate::commands::settings::stt_engine(app) == "openai" {
        let api_key = crate::commands::settings::get_openai_key_for_request(app).unwrap_or_default();
        if api_key.is_empty() {
            return Err("Voice is set to transcribe with your OpenAI key, but no OpenAI key is connected. Add one in Settings, or switch Voice to on this computer.".into());
        }
        return Ok(Engine::OpenAi { api_key });
    }
    if model::is_downloading() {
        return Err("The voice model is still downloading. Voice will work as soon as it finishes.".into());
    }
    let installed = model::model_dir(app).map(|d| model::is_installed(&d)).unwrap_or(false);
    if !installed {
        return Err(format!(
            "Voice needs a one-time {} MB download. Open Settings from the tray icon and choose Download under Voice.",
            model::total_size() / 1_000_000
        ));
    }
    Ok(Engine::Local)
}

fn controller(app: AppHandle, rx: Receiver<HookEvent>) {
    loop {
        let Ok(event) = rx.recv() else { return };
        if event != HookEvent::ChordDown || !crate::commands::settings::voice_enabled(&app) {
            continue;
        }

        let engine = resolve_engine(&app);
        // Open the mic now, before the engage delay, so the first word
        // isn't clipped. Closed again below if this turns out to be a
        // Windows shortcut rather than push-to-talk.
        let recorder = if engine.is_ok() { Some(audio::Recorder::start()) } else { None };

        let deadline = Instant::now() + ENGAGE_DELAY;
        let engaged = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Err(RecvTimeoutError::Timeout) => break true,
                Ok(HookEvent::ChordDown) => continue,
                Ok(_) => break false,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };
        if !engaged {
            continue; // recorder dropped here, mic closed
        }

        // From here on this is push-to-talk: keep releasing Win from
        // opening the Start menu.
        hotkey::mask_start_menu();

        let engine = match engine {
            Ok(e) => e,
            Err(message) => {
                emit_error(&app, message);
                continue;
            }
        };
        let recorder = match recorder {
            Some(Ok(r)) => r,
            Some(Err(e)) => {
                emit_error(&app, e);
                continue;
            }
            None => continue,
        };

        let session = match Session::begin(&app, recorder, engine) {
            Ok(s) => s,
            Err(e) => {
                emit_error(&app, e);
                continue;
            }
        };

        let released = loop {
            match rx.recv() {
                Ok(HookEvent::ChordUp) => break true,
                Ok(HookEvent::Interrupt) => break false,
                Ok(HookEvent::ChordDown) => continue,
                Err(_) => return,
            }
        };
        session.finish(released);
    }
}

// ---------------------------------------------------------------------
// One press
// ---------------------------------------------------------------------

#[derive(Clone, serde::Serialize)]
struct Level {
    level: f32,
}

#[derive(Clone, serde::Serialize)]
struct Partial {
    text: String,
}

#[derive(Clone, serde::Serialize)]
struct Final {
    text: String,
    /// "local" | "openai", for telemetry.
    engine: &'static str,
    /// How long the user spoke.
    duration_ms: u64,
    /// Release to transcript ready: the part of the wait voice adds.
    latency_ms: u64,
}

struct Session {
    app: AppHandle,
    recorder: audio::Recorder,
    stopping: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    engine_name: &'static str,
    worker: std::thread::JoinHandle<Result<String, String>>,
}

impl Session {
    fn begin(app: &AppHandle, recorder: audio::Recorder, engine: Engine) -> Result<Self, String> {
        // Talking over the previous answer's narration means "new question".
        let _ = crate::commands::tts::stop_speech(app.state());
        crate::capture_for_voice(app)?;

        let stopping = Arc::new(AtomicBool::new(false));
        let cancelled = Arc::new(AtomicBool::new(false));
        let engine_name = engine.name();
        let worker = {
            let app = app.clone();
            let audio = recorder.audio.clone();
            let stopping = stopping.clone();
            let cancelled = cancelled.clone();
            std::thread::spawn(move || transcribe_while_recording(&app, &engine, &audio, &stopping, &cancelled))
        };
        Ok(Self { app: app.clone(), recorder, stopping, cancelled, engine_name, worker })
    }

    fn finish(self, released: bool) {
        let released_at = Instant::now();
        if !released {
            self.cancelled.store(true, Ordering::SeqCst);
            drop(self.recorder);
            let _ = self.worker.join();
            let _ = self.app.emit("voice-cancelled", ());
            return;
        }

        let audio = self.recorder.audio.clone();
        // Mic closed first, then the transcriber told to finish, so its
        // final pass is guaranteed to see every recorded sample.
        self.recorder.stop();
        self.stopping.store(true, Ordering::SeqCst);

        match self.worker.join() {
            Ok(Ok(text)) => {
                let duration_ms = audio.lock().unwrap().len() as u64 * 1000 / segment::TARGET_RATE as u64;
                let latency_ms = released_at.elapsed().as_millis() as u64;
                if cfg!(debug_assertions) {
                    eprintln!("[voice] {} ms of speech, transcript ready {} ms after release", duration_ms, latency_ms);
                }
                let _ = self.app.emit("voice-final", Final { text, engine: self.engine_name, duration_ms, latency_ms });
            }
            Ok(Err(e)) => emit_error(&self.app, e),
            Err(_) => emit_error(&self.app, "Voice stopped unexpectedly."),
        }
    }
}

fn joined(parts: &[String]) -> String {
    parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(" ")
}

type Pending = std::thread::JoinHandle<Result<String, String>>;

fn spawn_transcription(app: &AppHandle, engine: &Engine, piece: Vec<f32>) -> Pending {
    let app = app.clone();
    let engine = engine.clone();
    std::thread::spawn(move || transcribe(&app, &engine, &piece))
}

/// Runs for the length of one press. Each finished phrase is handed off
/// for transcription as soon as the speaker pauses, without waiting for
/// the previous one: over the network (OpenAI) a phrase can take a couple
/// of seconds, and doing them one at a time would let a backlog build up
/// that release then has to wait through. Results are still assembled in
/// spoken order. (On-device, the model mutex serialises the work anyway.)
fn transcribe_while_recording(
    app: &AppHandle,
    engine: &Engine,
    audio: &audio::SharedAudio,
    stopping: &AtomicBool,
    cancelled: &AtomicBool,
) -> Result<String, String> {
    let mut cut = 0;
    let mut parts: Vec<String> = Vec::new();
    let mut pending: std::collections::VecDeque<Pending> = std::collections::VecDeque::new();

    let collect = |handle: Pending| -> Result<String, String> {
        handle.join().map_err(|_| "Transcription stopped unexpectedly.".to_string())?
    };

    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Ok(String::new()); // pending threads finish and are discarded
        }
        // Read the flag before the snapshot: once it's set the mic is
        // already closed, so this snapshot is the complete recording.
        let finishing = stopping.load(Ordering::SeqCst);
        let snapshot = audio.lock().unwrap().clone();

        if finishing {
            let tail = &snapshot[cut.min(snapshot.len())..];
            if !tail.is_empty() && !segment::is_silent(tail, &snapshot) {
                pending.push_back(spawn_transcription(app, engine, tail.to_vec()));
            }
            while let Some(handle) = pending.pop_front() {
                parts.push(collect(handle)?);
            }
            return Ok(joined(&parts));
        }

        let recent = &snapshot[snapshot.len().saturating_sub(1600)..];
        let _ = app.emit("voice-level", Level { level: segment::rms(recent) });

        while let Some(next) = segment::find_cut(&snapshot, cut) {
            let piece = &snapshot[cut..next];
            if !segment::is_silent(piece, &snapshot) {
                pending.push_back(spawn_transcription(app, engine, piece.to_vec()));
            }
            cut = next;
        }

        // Surface finished phrases in order as the live partial transcript.
        let mut progressed = false;
        while pending.front().is_some_and(|h| h.is_finished()) {
            parts.push(collect(pending.pop_front().unwrap())?);
            progressed = true;
        }
        if progressed {
            let _ = app.emit("voice-partial", Partial { text: joined(&parts) });
        }

        std::thread::sleep(TICK);
    }
}

// ---------------------------------------------------------------------
// Commands (Settings)
// ---------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct VoiceStatus {
    enabled: bool,
    /// "local" | "openai"
    engine: &'static str,
    openai_key_connected: bool,
    model_installed: bool,
    downloading: bool,
    model_size_mb: u64,
}

#[tauri::command]
pub fn get_voice_status(app: AppHandle) -> Result<VoiceStatus, String> {
    let dir = model::model_dir(&app)?;
    Ok(VoiceStatus {
        enabled: crate::commands::settings::voice_enabled(&app),
        engine: crate::commands::settings::stt_engine(&app),
        openai_key_connected: crate::commands::settings::get_openai_key_status(app.clone()).unwrap_or(false),
        model_installed: model::is_installed(&dir),
        downloading: model::is_downloading(),
        model_size_mb: model::total_size() / 1_000_000,
    })
}

#[tauri::command]
pub async fn download_voice_model(app: AppHandle) -> Result<(), String> {
    model::download(app.clone()).await?;
    preload(&app);
    Ok(())
}

#[tauri::command]
pub fn set_voice_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    crate::commands::settings::persist_voice_enabled(&app, enabled)?;
    if enabled {
        preload(&app);
    } else {
        // Frees the ~1 GB the loaded model holds.
        *ENGINE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    Ok(())
}

#[tauri::command]
pub fn set_stt_engine(app: AppHandle, engine: String) -> Result<(), String> {
    crate::commands::settings::set_stt_engine(app.clone(), engine.clone())?;
    if engine == "local" {
        preload(&app);
    } else {
        // Not needed while transcribing with OpenAI; frees ~1 GB.
        *ENGINE.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    Ok(())
}
