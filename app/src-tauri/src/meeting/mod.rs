//! Meeting mode (experimental): a running transcript of the call, and one
//! hotkey that answers the question just asked.
//!
//! 1. Ctrl+Alt+L starts a session (or the first free fallback key, see
//!    `TOGGLE_CANDIDATES`). System audio (what the other people in
//!    the call say, via loopback of the default output device) is recorded
//!    and transcribed phrase by phrase, on-device by default, into an
//!    in-memory transcript. The user's own mic is not recorded.
//! 2. Ctrl+Alt+Enter: whatever has been said since the last pause is
//!    transcribed right away, and the last few minutes of transcript go to
//!    the backend, which streams back a suggested answer. Because the
//!    question was transcribed while it was being asked, the wait is mostly
//!    the model's time to first token.
//! 3. The answer shows in a small panel at the top of the screen that is
//!    always excluded from screen capture and never takes focus.
//! 4. The same key again ends the session: audio closes, the transcript is
//!    dropped. Nothing is written to disk at any point.
//!
//! While a session runs, Ctrl+Alt+H hides or shows the panel and
//! Ctrl+Alt+Up/Down scrolls it. Those are registered only for the length
//! of a session so they don't take keys from other apps the rest of the
//! time. Narration (TTS) is refused during a session: it would play into
//! the call and be picked up by the loopback.

pub mod answer;
pub mod chunker;
pub mod transcript;

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

use crate::stealth::{self, Protection};
use crate::voice::{self, audio, segment};
use chunker::Chunker;
use transcript::Transcript;

/// How often the transcriber looks for finished phrases.
const TICK: Duration = Duration::from_millis(100);

/// No new audio for this long counts as a pause. WASAPI loopback delivers
/// no samples at all while nothing is playing, so silence after a phrase
/// often never arrives as quiet samples for the pause detector to see.
const STALL: Duration = Duration::from_millis(600);

/// Upper bound on buffered, not-yet-transcribed audio. The chunker drains
/// the buffer continuously, so this is only reached if transcription falls
/// far behind; past it, new audio is dropped rather than memory growing.
const BUFFER_SECONDS: usize = 120;

/// How long to wait before retrying when the output device can't be opened
/// (e.g. while switching headsets).
const REOPEN_DELAY: Duration = Duration::from_secs(2);

/// Minimum spacing between non-fatal transcription error toasts.
const ERROR_COOLDOWN: Duration = Duration::from_secs(15);

static ACTIVE: AtomicBool = AtomicBool::new(false);
static SESSION: Mutex<Option<Running>> = Mutex::new(None);
static PANEL_VISIBLE: AtomicBool = AtomicBool::new(false);
static PANEL_PROTECTION: Mutex<Option<Protection>> = Mutex::new(None);

struct Running {
    ctrl: Sender<Ctrl>,
    thread: std::thread::JoinHandle<()>,
    transcript: Arc<Mutex<Transcript>>,
}

pub(crate) enum Ctrl {
    Stop,
    /// Transcribe everything recorded so far, then reply.
    Flush(Sender<()>),
}

pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// The session's control channel and transcript, for the answer path.
pub(crate) fn session_handles() -> Option<(Sender<Ctrl>, Arc<Mutex<Transcript>>)> {
    let guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    guard.as_ref().map(|r| (r.ctrl.clone(), r.transcript.clone()))
}

// ---------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------

#[derive(Clone, serde::Serialize)]
struct StatePayload {
    /// "listening" | "stopped" | "reconnecting" | "error"
    state: &'static str,
    message: Option<String>,
}

pub(crate) fn emit_state(app: &AppHandle, state: &'static str, message: Option<String>) {
    let _ = app.emit("meeting-state", StatePayload { state, message });
}

#[derive(Clone, serde::Serialize)]
struct Level {
    level: f32,
}

#[derive(Clone, serde::Serialize)]
struct Heard {
    text: String,
    lines: usize,
}

// ---------------------------------------------------------------------
// Panel window
// ---------------------------------------------------------------------

fn panel(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(stealth::MEETING_WINDOW)
}

/// Run once at startup: click-through and top-centre of the primary monitor.
/// It never takes focus (`focusable: false` in tauri.conf.json) and is
/// always excluded from capture (`stealth::init` hooks it to be protected
/// whenever it's shown).
pub fn setup_panel(app: &AppHandle) {
    let Some(window) = panel(app) else { return };
    let _ = window.set_ignore_cursor_events(true);
    if let (Ok(Some(monitor)), Ok(size)) = (app.primary_monitor(), window.outer_size()) {
        let pos = monitor.position();
        let area = monitor.size();
        let margin = (10.0 * monitor.scale_factor()) as i32;
        let x = pos.x + (area.width as i32 - size.width as i32) / 2;
        let _ = window.set_position(tauri::PhysicalPosition::new(x, pos.y + margin));
    }
    let protection = stealth::panel_protection();
    *PANEL_PROTECTION.lock().unwrap_or_else(|e| e.into_inner()) = Some(protection);
    if protection != Protection::Hidden {
        eprintln!("[meeting] panel capture protection: {:?}", protection);
    }
}

fn show_panel(app: &AppHandle) {
    if let Some(window) = panel(app) {
        stealth::show_without_focus(&window);
        PANEL_VISIBLE.store(true, Ordering::SeqCst);
    }
}

fn hide_panel(app: &AppHandle) {
    if let Some(window) = panel(app) {
        let _ = window.hide();
        PANEL_VISIBLE.store(false, Ordering::SeqCst);
    }
}

pub(crate) fn reveal_panel(app: &AppHandle) {
    if !PANEL_VISIBLE.load(Ordering::SeqCst) {
        show_panel(app);
    }
}

// ---------------------------------------------------------------------
// Hotkeys
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Answer,
    TogglePanel,
    Scroll(i32),
}

const CTRL_ALT: Modifiers = Modifiers::CONTROL.union(Modifiers::ALT);

/// Keys tried, in order, for starting and ending a session: another app
/// (a call or recording tool, say) may already own the first. All three
/// keys, so it stays easy to press mid-call. Ctrl+Alt+M is no longer first
/// because ParakeetAI takes it; Ctrl+Shift+M is left out on purpose, since
/// Teams uses it to mute.
const TOGGLE_CANDIDATES: [(Modifiers, Code, &str); 3] = [
    (CTRL_ALT, Code::KeyL, "Ctrl+Alt+L"),
    (CTRL_ALT, Code::KeyJ, "Ctrl+Alt+J"),
    (CTRL_ALT, Code::KeyM, "Ctrl+Alt+M"),
];

/// The candidate that registered, if any; what `action_for` matches against.
static TOGGLE: Mutex<Option<(Shortcut, &'static str)>> = Mutex::new(None);

/// Registers the first free toggle key, for the life of the app, and returns
/// its name. Run once at startup, before anything that shows the name.
pub fn register_toggle(app: &AppHandle) -> Option<&'static str> {
    for (modifiers, code, label) in TOGGLE_CANDIDATES {
        let shortcut = Shortcut::new(Some(modifiers), code);
        match app.global_shortcut().register(shortcut) {
            Ok(()) => {
                *TOGGLE.lock().unwrap_or_else(|e| e.into_inner()) = Some((shortcut, label));
                if cfg!(debug_assertions) {
                    eprintln!("[meeting] toggle key: {}", label);
                }
                return Some(label);
            }
            Err(e) => eprintln!("[meeting] couldn't register {}: {}", label, e),
        }
    }
    None
}

/// The key that starts and ends a session, or None if every candidate was taken.
pub fn toggle_key() -> Option<&'static str> {
    TOGGLE.lock().unwrap_or_else(|e| e.into_inner()).map(|(_, label)| label)
}

/// Registered only while a session runs.
fn session_shortcuts() -> [(Shortcut, Action); 4] {
    [
        (Shortcut::new(Some(CTRL_ALT), Code::Enter), Action::Answer),
        (Shortcut::new(Some(CTRL_ALT), Code::KeyH), Action::TogglePanel),
        (Shortcut::new(Some(CTRL_ALT), Code::ArrowUp), Action::Scroll(-1)),
        (Shortcut::new(Some(CTRL_ALT), Code::ArrowDown), Action::Scroll(1)),
    ]
}

pub fn action_for(shortcut: &Shortcut) -> Option<Action> {
    if TOGGLE.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|(s, _)| &s == shortcut) {
        return Some(Action::Toggle);
    }
    session_shortcuts().into_iter().find(|(s, _)| s == shortcut).map(|(_, a)| a)
}

pub fn handle(app: &AppHandle, action: Action) {
    match action {
        Action::Toggle => toggle(app),
        Action::Answer => answer::answer_now(app),
        Action::TogglePanel => {
            if PANEL_VISIBLE.load(Ordering::SeqCst) {
                hide_panel(app);
            } else {
                show_panel(app);
            }
        }
        Action::Scroll(dir) => {
            let _ = app.emit("meeting-scroll", dir);
        }
    }
}

fn register_session_shortcuts(app: &AppHandle) {
    let mut failed = Vec::new();
    for (shortcut, action) in session_shortcuts() {
        if let Err(e) = app.global_shortcut().register(shortcut) {
            eprintln!("[meeting] couldn't register {:?}: {}", action, e);
            failed.push(match action {
                Action::Answer => "Ctrl+Alt+Enter",
                Action::TogglePanel => "Ctrl+Alt+H",
                Action::Scroll(_) => "Ctrl+Alt+Up/Down",
                Action::Toggle => "the meeting key",
            });
        }
    }
    failed.dedup();
    if !failed.is_empty() {
        emit_state(
            app,
            "listening",
            Some(format!("Another app already uses {}. Those keys won't work in this meeting.", failed.join(", "))),
        );
    }
}

fn unregister_session_shortcuts(app: &AppHandle) {
    for (shortcut, _) in session_shortcuts() {
        let _ = app.global_shortcut().unregister(shortcut);
    }
}

// ---------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------

pub fn toggle(app: &AppHandle) {
    if is_active() {
        stop(app);
    } else {
        start(app);
    }
}

fn fail_to_start(app: &AppHandle, message: String) {
    show_panel(app);
    emit_state(app, "error", Some(message));
}

pub fn start(app: &AppHandle) {
    let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_some() {
        return;
    }

    let engine = match voice::resolve_engine(app) {
        Ok(e) => e,
        Err(message) => return fail_to_start(app, format!("Meeting mode needs speech-to-text. {}", message)),
    };
    let recorder = match audio::Recorder::start_from(audio::Source::System, BUFFER_SECONDS) {
        Ok(r) => r,
        Err(message) => return fail_to_start(app, message),
    };

    voice::warm_up(app);
    let _ = crate::commands::tts::stop_speech(app.state());

    let transcript = Arc::new(Mutex::new(Transcript::new()));
    let (ctrl, rx) = mpsc::channel();
    let thread = {
        let app = app.clone();
        let transcript = transcript.clone();
        std::thread::Builder::new()
            .name("pointr-meeting".into())
            .spawn(move || transcriber(app, engine, recorder, transcript, rx))
            .expect("failed to spawn meeting transcriber")
    };

    *guard = Some(Running { ctrl, thread, transcript });
    ACTIVE.store(true, Ordering::SeqCst);
    drop(guard);

    show_panel(app);
    emit_state(app, "listening", None);
    register_session_shortcuts(app);
    if cfg!(debug_assertions) {
        eprintln!("[meeting] session started");
    }
}

pub fn stop(app: &AppHandle) {
    let running = SESSION.lock().unwrap_or_else(|e| e.into_inner()).take();
    ACTIVE.store(false, Ordering::SeqCst);
    answer::cancel();
    unregister_session_shortcuts(app);
    hide_panel(app);
    emit_state(app, "stopped", None);

    if let Some(running) = running {
        let _ = running.ctrl.send(Ctrl::Stop);
        // Joined off the hotkey thread: a transcription in flight can take
        // a moment to finish, and its result is thrown away anyway.
        std::thread::spawn(move || {
            let _ = running.thread.join();
            if cfg!(debug_assertions) {
                eprintln!("[meeting] session ended, transcript dropped");
            }
        });
    }
}

// ---------------------------------------------------------------------
// Transcriber
// ---------------------------------------------------------------------

type Pending = std::thread::JoinHandle<Result<String, String>>;

fn spawn_transcription(app: &AppHandle, engine: &voice::Engine, piece: Vec<f32>) -> Pending {
    let app = app.clone();
    let engine = engine.clone();
    std::thread::spawn(move || {
        // Push-to-talk shares the on-device model and is the user waiting
        // on an answer right now; meeting audio can wait a moment.
        while voice::PTT_ACTIVE.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(50));
        }
        voice::transcribe(&app, &engine, &piece)
    })
}

/// Runs for the length of a session: cuts finished phrases out of the
/// system-audio buffer, transcribes them in order, and appends them to the
/// transcript. A `Flush` forces out everything recorded so far and replies
/// once it's all in the transcript.
fn transcriber(
    app: AppHandle,
    engine: voice::Engine,
    recorder: audio::Recorder,
    transcript: Arc<Mutex<Transcript>>,
    rx: Receiver<Ctrl>,
) {
    let started = Instant::now();
    let mut recorder = Some(recorder);
    let mut chunker = Chunker::new();
    let mut pending: VecDeque<(f64, Pending)> = VecDeque::new();
    let mut waiters: Vec<Sender<()>> = Vec::new();
    let mut last_len = 0usize;
    let mut last_growth = Instant::now();
    let mut last_error: Option<Instant> = None;

    let finish = |t: f64, handle: Pending, last_error: &mut Option<Instant>| match handle.join() {
        Ok(Ok(text)) => {
            let mut tr = transcript.lock().unwrap_or_else(|e| e.into_inner());
            if tr.push(t, &text) {
                let _ = app.emit("meeting-heard", Heard { text, lines: tr.len() });
            }
        }
        Ok(Err(e)) => {
            eprintln!("[meeting] transcription failed: {}", e);
            if last_error.is_none_or(|at| at.elapsed() >= ERROR_COOLDOWN) {
                *last_error = Some(Instant::now());
                emit_state(&app, "listening", Some(e));
            }
        }
        Err(_) => eprintln!("[meeting] transcription thread panicked"),
    };

    loop {
        let mut flush = false;
        match rx.recv_timeout(TICK) {
            Ok(Ctrl::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Ctrl::Flush(reply)) => {
                waiters.push(reply);
                flush = true;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }

        // The stream died (device unplugged, stream invalidated): open
        // whatever the default output is now. A change of default device
        // doesn't land here; cpal moves the stream over by itself.
        if recorder.as_ref().is_none_or(|r| r.failed.load(Ordering::SeqCst)) {
            if recorder.take().is_some() {
                emit_state(&app, "reconnecting", None);
            }
            match audio::Recorder::start_from(audio::Source::System, BUFFER_SECONDS) {
                Ok(r) => {
                    recorder = Some(r);
                    chunker.reset();
                    last_len = 0;
                    last_growth = Instant::now();
                    emit_state(&app, "listening", None);
                }
                Err(e) => {
                    emit_state(&app, "reconnecting", Some(e));
                    for w in waiters.drain(..) {
                        let _ = w.send(());
                    }
                    if let Ok(Ctrl::Stop) | Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(REOPEN_DELAY) {
                        break;
                    }
                    continue;
                }
            }
        }
        let Some(rec) = recorder.as_ref() else { continue };

        let now = started.elapsed().as_secs_f64();
        let pieces = {
            let mut buf = rec.audio.lock().unwrap_or_else(|e| e.into_inner());
            let len = buf.len();
            if len != last_len {
                last_growth = Instant::now();
            }
            let stalled = last_growth.elapsed() >= STALL;
            let level = if stalled { 0.0 } else { segment::rms(&buf[len.saturating_sub(1600)..]) };
            let _ = app.emit_to(stealth::MEETING_WINDOW, "meeting-level", Level { level });

            let pieces = chunker.take(&mut buf, flush || stalled);
            last_len = buf.len();
            pieces
        };
        for piece in pieces {
            pending.push_back((now, spawn_transcription(&app, &engine, piece)));
        }

        // Add finished phrases in spoken order; on a flush, wait for all.
        while pending.front().is_some_and(|(_, h)| h.is_finished() || !waiters.is_empty()) {
            let (t, handle) = pending.pop_front().unwrap();
            finish(t, handle, &mut last_error);
        }
        for w in waiters.drain(..) {
            let _ = w.send(());
        }
    }
    // Recorder closes on drop; anything still transcribing is discarded.
}

// ---------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------

#[derive(serde::Serialize)]
pub struct MeetingStatus {
    active: bool,
    /// The panel's capture protection, as set at startup.
    protection: Option<Protection>,
    lines: usize,
    /// The key that starts and ends a session, for the UI to show. None if
    /// no candidate could be registered (the tray menu still works).
    toggle_key: Option<&'static str>,
}

#[tauri::command]
pub fn get_meeting_status() -> MeetingStatus {
    let lines = session_handles()
        .map(|(_, t)| t.lock().unwrap_or_else(|e| e.into_inner()).len())
        .unwrap_or(0);
    MeetingStatus {
        active: is_active(),
        protection: *PANEL_PROTECTION.lock().unwrap_or_else(|e| e.into_inner()),
        lines,
        toggle_key: toggle_key(),
    }
}

#[tauri::command]
pub fn toggle_meeting(app: AppHandle) -> MeetingStatus {
    toggle(&app);
    get_meeting_status()
}
