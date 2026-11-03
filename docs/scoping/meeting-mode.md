# Scope: Stealth overlay + Meeting copilot

Status: **experimental, on this branch only, not for release.**
Branch: `claude/jolly-goldberg-ncov7s`

## Implementation status

Decisions taken after the first draft of this doc:
- Running transcript with an "answer now" hotkey (not record-while-held).
- **System audio only.** The user's mic isn't transcribed (we only need the question).
- No screenshot in meeting answers for now.
- Stays on this branch; not shipped.

Built:

| Piece | Where |
|---|---|
| Stealth toggle: `WDA_EXCLUDEFROMCAPTURE` with read-back, `WDA_MONITOR` fallback, hidden from taskbar/Alt-Tab while on | `src-tauri/src/stealth.rs`, Settings → "Hide Pointr from screen sharing" |
| Loopback capture of the default output device | `voice/audio.rs` (`Source::System`, `Recorder::start_from`) |
| Continuous phrase cutting with drain, stall-as-pause for silent loopback | `meeting/chunker.rs` (unit tested) |
| In-memory transcript, 15 min kept, last 4 min / 8k chars sent | `meeting/transcript.rs` (unit tested) |
| Session control, transcriber thread, device-loss reconnect, PTT priority | `meeting/mod.rs` |
| Answer: flush tail → POST → stream into panel; newer press cancels older | `meeting/answer.rs` |
| Panel: always capture-excluded, non-activating, click-through, top-centre | `meeting.html`/`meeting.js`, `tauri.conf.json` window `meeting` |
| Narration (TTS) never plays while stealth is on or a meeting session runs, enforced in `speak_text` for every caller; turning stealth on stops anything mid-sentence; Settings shows "Read answers aloud" locked off | `commands/tts.rs` (`narration_blocked`), `stealth.rs`, `settings.js` |
| Backend `POST /api/meeting/answer-stream` (text-only, stateless) | `backend/app/routes/meeting.py`, tests in `backend/tests/test_meeting.py` |
| Meeting brief (free-text background) | Settings → Meeting mode |

Hotkeys: `Ctrl+Alt+M` start/end (also in the tray menu). During a session only: `Ctrl+Alt+Enter` answer, `Ctrl+Alt+H` hide/show panel, `Ctrl+Alt+Up/Down` scroll.

Not built yet: output-device picker (uses the Windows default), per-process loopback, panel move/resize, speculative answering, follow-ups by voice, telemetry, privacy-page update.

How it was checked: the Rust crate type-checks and passes clippy against the Windows target (`x86_64-pc-windows-gnu`, cross-checked from Linux); the pure modules' unit tests pass; backend route tests pass. **Not yet run on a real Windows machine.** Everything below still needs a manual pass there: the capture-exclusion matrix (Zoom / Teams / Meet in Chrome and Edge / OBS / Snipping Tool), loopback on speakers vs. Bluetooth headset, and real answer latency.

To try it: Settings → Voice set up (local model downloaded, or OpenAI key), an LLM key connected, optionally the meeting background filled in → join a call → `Ctrl+Alt+M` → when asked something, `Ctrl+Alt+Enter`. Debug builds (`npm run tauri dev`) log flush time and time to first token as `[meeting] answer N: ...`.

---

Two features that ship well together but should be built and toggled separately:

1. **Stealth mode**: Pointr's windows don't appear in screen shares, recordings or screenshots, but stay visible on the user's own monitor.
2. **Meeting copilot**: while a meeting is running, Pointr keeps a live transcript of the call. One hotkey turns "the question they just asked" into an answer on screen, streamed, in about a second.

Stealth is small and mostly useful on its own (answers no longer show up when you share your screen). The meeting copilot is the larger piece and depends on stealth to be usable in a call.

---

## 1. What Cluely / Parakeet AI actually do (behaviour to match)

| Behaviour | How they do it | Pointr equivalent |
|---|---|---|
| Overlay invisible in Zoom/Meet/Teams shares | `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on Windows, `NSWindow.sharingType = .none` on macOS | Same Win32 call; Tauri already wraps it (see 2.1) |
| Not in taskbar / Alt-Tab | Tool window, skip taskbar | `skipTaskbar` already set on the overlay; add `WS_EX_TOOLWINDOW` |
| Doesn't steal focus from the meeting | Non-activating window, driven by global hotkeys | New: `WS_EX_NOACTIVATE` panel and hotkeys only (2.3) |
| Listens to the call | System-audio loopback (what the others say) plus mic (what you say) | cpal already supports WASAPI loopback (3.2) |
| "What should I say?" hotkey | Rolling transcript, so the question is already transcribed when you press | Rolling transcript during a meeting session (3.1) |
| Small panel near the webcam | Floating translucent card, top-centre, so your eyes stay near the camera | New `meeting` window, not the full-screen overlay (3.5) |
| Optional screen context | Screenshot sent with the transcript | Existing capture pipeline, opt-in per press |

---

## 2. Stealth mode (hide from screen share)

### 2.1 Mechanism

`SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` (Windows 10 2004 / build 19041+). The window is drawn on the physical display but removed from every frame the OS hands to capture APIs, including Windows Graphics Capture, Desktop Duplication and GDI `BitBlt` of the desktop. Zoom, Teams, Meet/any browser `getDisplayMedia`, Discord, OBS, Snipping Tool and Game Bar all go through those APIs.

Tauri 2 exposes this as `WebviewWindow::set_content_protected(true)` / `"contentProtected": true` in `tauri.conf.json`. Checked against tao 0.35.3 (the version in our `Cargo.lock`):

```rust
// tao/src/platform_impl/windows/window.rs
pub fn set_content_protection(&self, enabled: bool) {
    unsafe {
        let _ = SetWindowDisplayAffinity(self.hwnd(),
            if enabled { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    }
}
```

**Gotcha: the result is discarded.** Pointr's stated minimum is Windows 10 **1903**. On 1903/1909, `WDA_EXCLUDEFROMCAPTURE` fails, the call silently does nothing, and the window stays **fully visible** in the share. So we call it ourselves:

1. `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`
2. Read it back with `GetWindowDisplayAffinity`. If that didn't take, fall back to `WDA_MONITOR` (the window shows as a black box in the share, which is not hidden but at least doesn't leak content), and tell the user stealth isn't fully supported on their build.

### 2.2 What it does and doesn't hide

Hides from: software capture on the same machine (meeting apps, recorders, screenshot tools, most remote-desktop tools).

Does **not** hide from:
- A phone or camera pointed at the screen, or HDMI capture cards.
- **The mouse cursor.** If you hover or click the hidden panel, viewers see a cursor moving over "nothing". Meeting mode should be keyboard-only and click-through.
- **Focus changes.** Clicking or typing into Pointr takes focus away from the meeting window or browser tab. Some web apps log `blur`/`visibilitychange`.
- **The tray icon** when sharing the entire screen (it's drawn by Explorer, not by us).
- Software that **looks for** excluded windows: any process can enumerate windows and call `GetWindowDisplayAffinity`, and some proctoring and interview platforms already flag Cluely-style tools that way. This is a hard limit and we should state it plainly rather than try to work around it (see 5).

A nice side effect: once the overlay is excluded, Pointr's own GDI screenshots (`capture/screen.rs`) no longer include Pointr's own answer bubble. Today that matters for the multi-step agent loop, which re-captures the screen while the overlay may be up. (Verify in the spike: excluded windows should be omitted from desktop `BitBlt` on 2004+.)

### 2.3 Changes

| Where | Change |
|---|---|
| `src-tauri/src/stealth.rs` (new) | `apply(window, enabled) -> StealthResult { Full, BlackBoxOnly, Unsupported }` using `SetWindowDisplayAffinity` + `GetWindowDisplayAffinity`; also toggles `WS_EX_TOOLWINDOW` (hides from Alt-Tab) and `skip_taskbar` |
| `lib.rs` `setup` | Apply to **every** window (`splash`, `main`, `settings`, `history`, and later `meeting`) at startup and when the setting changes. Windows created later must apply it on creation |
| `commands/settings.rs` | `stealth_enabled: bool` in `PersistedSettings`, plus `get/set_stealth_enabled` commands. Default **off** |
| `settings.html/js` | Toggle "Hide Pointr from screen sharing and recordings", with the result shown ("Fully hidden" / "Shows as a black box on this Windows version") |
| Tray menu | "Stealth: On/Off" item, and optionally "Hide tray icon while stealth is on" (Settings then has to be reachable by hotkey) |
| `overlay/window.rs` | No change for the normal Ctrl+Alt+Space flow (you type there, so it needs focus). The meeting panel is a separate window |
| Windows `windows` crate features | `Win32_UI_WindowsAndMessaging` is already enabled, so `SetWindowDisplayAffinity` and `SetWindowLongPtrW` are available |

Estimate: **1–2 days** including a manual test matrix (Zoom, Teams, Meet in Chrome/Edge, Discord, OBS, Snipping Tool, Win+Shift+S, Game Bar) on Windows 10 22H2 and Windows 11.

---

## 3. Meeting copilot

### 3.1 Interaction model: rolling transcript, not record-on-press

"Press the hotkey, record the meeting audio, then answer" has a timing problem. By the time you know you need help, the question has **already been asked**. If recording only starts on the press, it misses the question.

So the copilot runs as an explicit **meeting session**:

1. **Start meeting** (hotkey `Ctrl+Alt+M`, or tray). A small status pill confirms it's listening (only you see it, since it's stealth).
2. While the session runs, system audio and mic are transcribed continuously into an in-memory transcript (last ~10 min, RAM only, never written to disk).
3. **Answer now** (`Ctrl+Alt+Enter`): the transcript tail is flushed, sent with the user's meeting brief, and the answer streams into the panel. You don't wait for any audio recording at press time, because it has already happened.
4. **End meeting** (`Ctrl+Alt+M` again): mic and loopback close, and the transcript is dropped from memory.

Push-to-talk (Ctrl+Win) keeps working inside a session for follow-ups like "shorter", "give me an example" or "what about edge cases".

A push-to-capture variant (hold the hotkey while *they* talk) is cheap to add on top of the same plumbing, but it shouldn't be the default.

### 3.2 Audio capture

| Stream | Source | Label in transcript |
|---|---|---|
| Others | WASAPI **loopback** of the default render device (what comes out of your speakers/headset) | `THEM` |
| You | Default mic (already implemented in `voice/audio.rs`) | `ME` |

Two separate streams give a basic "who spoke" split without running a diarization model.

cpal 0.18.2 (our version) already does loopback: building an **input** stream on an **output** device sets `AUDCLNT_STREAMFLAGS_LOOPBACK` (`src/host/wasapi/device.rs`). So `voice/audio.rs` becomes:

```rust
pub enum Source { Mic, Loopback }
Recorder::start(source: Source, buffer: Buffer)   // Buffer::Capped(60s) | Buffer::Ring(secs)
// Loopback: host.default_output_device() -> build_input_stream(...)
```

The existing callback (convert to f32, mono, resample to 16 kHz) is reused unchanged.

Gotchas:
- **Wrong device.** If Teams outputs to a headset while the Windows default is the speakers, loopback on the default device captures silence. Needs an output-device picker in Settings, or capture of all active render endpoints.
- **No headphones = echo.** The mic hears the speakers, so `THEM` lines also show up as `ME`. Mitigation: when a mic phrase overlaps in time with a loopback phrase with high text similarity, drop the mic copy. Recommend headphones in the UI.
- **Pointr's own TTS** would be captured by loopback **and heard by everyone in the meeting**. Narration must be force-disabled while a session runs (`commands/tts.rs`, check in `speak_text`).
- **Everything else on the speakers** (Spotify, notification sounds) ends up in the transcript. Phase 3: per-process loopback (`ActivateAudioInterfaceAsync` with `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`, Windows build 20348+ / Windows 11) captures only `Zoom.exe` / `ms-teams.exe` / the browser. Not in cpal; needs a hand-rolled WASAPI path via the `windows` crate (`Win32_Media_Audio` feature).

### 3.3 Transcription

Reuse the Parakeet pipeline (`voice/mod.rs` + `voice/segment.rs`):

- `segment::find_cut` already splits on pauses. A new `meeting::transcriber` thread does what `transcribe_while_recording` does, but runs indefinitely on a ring buffer and appends `{speaker, t_start, t_end, text}` lines to a shared `Transcript`.
- **Model contention:** `ENGINE` is one mutex-guarded `ParakeetModel`. Two streams plus push-to-talk all queue on it. That's fine at real-time rates (int8 0.6B on CPU runs well faster than real time, to be measured), but push-to-talk must be **prioritised** or it'll feel slow mid-meeting. A simple fix: the meeting transcriber `try_lock`s and yields when PTT is active.
- **CPU and battery:** continuous transcription keeps a core busy for the whole call. Measure on a mid-range laptop. Options if it's heavy: transcribe `THEM` only by default (that's where the questions come from), and make `ME` opt-in.
- **Cloud STT option:** `voice/remote.rs` (OpenAI under the user's key) works phrase by phrase too. For a 1-hour meeting it costs about $0.20–0.40 at current transcribe-model pricing. Keep local as the default.
- **Languages:** Parakeet v3 covers English and 24 European languages. Anything else needs the OpenAI engine.

### 3.4 Answer generation (backend)

New route in `backend/app/routes/` (e.g. `meeting.py`): `POST /api/meeting/answer-stream`, NDJSON, same shape as `/analyze-screen-stream` (`{"type":"chunk"}` … `{"type":"done"}`) so the client streaming code is reused.

Request:
```jsonc
{
  "transcript": [{"speaker": "THEM", "text": "...", "t": 812.4}, ...], // last ~3–5 min
  "brief": "Role: backend engineer. Talking to: ... Notes: ...",        // user-written, stored locally
  "screenshot_base64": null,     // only if the user pressed the "with screen" variant
  "follow_up": null,             // PTT text, e.g. "shorter"
  "provider": "...", "model": "...", "gemini_api_key": "...", "openai_api_key": "..."
}
```

Prompt outline: *"Live meeting transcript. THEM = other participants, ME = the user. Find the most recent question or request directed at ME that ME hasn't fully answered. Reply with what ME could say: lead with the direct answer in 1–2 sentences, then up to 3 short bullets. No preamble. If there is no question, summarise the last exchange in one line and suggest a next thing ME could say."*

Needed in services: a **text-only** `analyze_stream` (today `gemini.analyze_stream(image_bytes, prompt)` requires an image; `openai_service._user_input` already accepts `None`). Gemini needs the same `image_bytes: bytes | None` path.

Privacy: do **not** call `record_exchange(...)` / session memory with meeting transcripts. Keep the relay stateless for this route, and update `landing/privacy.html` to describe what's sent.

### 3.5 Display: the meeting panel

A new `meeting` window in `tauri.conf.json`: ~420×220, transparent, `alwaysOnTop`, `skipTaskbar`, `focus: false`, `contentProtected: true`, positioned top-centre under the webcam by default.

- Non-activating (`WS_EX_NOACTIVATE`) and click-through (`set_ignore_cursor_events(true)`), so it never takes focus or shows a cursor interaction on the share.
- Keyboard only: `Ctrl+Alt+Enter` answer, `Ctrl+Alt+↑/↓` scroll, `Ctrl+Alt+←/→` previous/next answer, `Ctrl+Alt+Arrow+Shift` move panel, `Ctrl+Alt+H` hide/show. All through `tauri_plugin_global_shortcut`, which doesn't need focus.
- States: *Listening* (with a level meter from the existing `voice-level` events), *Thinking*, *Streaming answer*, *Error*. Shows the detected question above the answer, so a wrong guess is obvious immediately.
- New `src/meeting.html` + `meeting.js`. Reuse the stream-chunk rendering from `main.js`.

### 3.6 Latency budget (press → first words on screen)

| Step | Target | Notes |
|---|---|---|
| Flush transcript tail (audio since the last pause) | 150–400 ms | Usually ≤ 3 s of audio. Local Parakeet. Measure |
| Client → relay → provider | 50–150 ms | One extra hop vs. calling the provider directly |
| Model time-to-first-token | 300–900 ms | Flash/mini-class models. Show a hint in Settings that large models are slow here |
| **Total to first token** | **≈ 0.6–1.5 s** | |

Later optimisations (Phase 3):
- **Speculative answering:** when a `THEM` phrase ends and looks like a question (ends in a question, starts with "how/what/why/can you/tell me…"), start the request in the background. When the user presses the hotkey, the answer is already streaming or done. That costs tokens on questions nobody asks for help with, so cap it per minute.
- **Direct-to-provider** from Rust for this one route (skip the relay). It saves the hop but splits provider logic across two codebases, so do it only if measurements say the hop matters.
- **Auto-answer mode:** show answers without a press. This is what Cluely does by default. It's noisy, so keep it off by default.

### 3.7 Code touchpoints

```
app/src-tauri/src/
  stealth.rs                 NEW  display affinity + tool-window style
  meeting/mod.rs             NEW  session start/stop, hotkeys, state
  meeting/transcript.rs      NEW  ring of {speaker, t, text}, tail flush, echo de-dupe
  meeting/answer.rs          NEW  POST /api/meeting/answer-stream, forward chunks to the panel
  voice/audio.rs             Source::{Mic, Loopback}, ring-buffer mode
  voice/mod.rs               expose transcribe(); PTT priority over the meeting transcriber
  commands/settings.rs       stealth_enabled, meeting_brief, meeting_output_device, meeting_transcribe_me
  commands/tts.rs            refuse to speak while a meeting session is active
  lib.rs                     register hotkeys, window setup, tray items
  tauri.conf.json            "meeting" window
app/src/
  meeting.html / meeting.js  NEW  panel UI
  settings.html / settings.js  Stealth + Meeting sections, brief editor, device picker
backend/app/
  routes/meeting.py          NEW  /api/meeting/answer-stream
  models/meeting.py          NEW  request schema
  services/gemini.py         text-only stream
  main.py                    include router
landing/privacy.html         what meeting mode sends, and that nothing is stored
```

---

## 4. Phasing

| Phase | Scope | Estimate |
|---|---|---|
| **0. Spike** (1–2 days) | (a) `set_content_protected` + affinity read-back on all windows, run against the capture-app matrix. (b) cpal loopback → Parakeet → print transcript lines live, and measure CPU and real-time factor. (c) A hard-coded transcript sent to a text-only stream endpoint, to measure time to first token per model. | 1–2 d |
| **1. Stealth mode** | Section 2, behind a Settings toggle. Shippable on its own. | 1–2 d |
| **2. Copilot MVP** | Meeting session, loopback + mic, rolling transcript, `Ctrl+Alt+Enter` answer, meeting panel, brief, TTS lockout, backend route, privacy page | 5–8 d |
| **3. Polish** | Output-device picker, echo de-dupe, per-process loopback, speculative answering, optional screenshot variant, auto-answer, telemetry (`meeting_started`, `meeting_answer` with latency only, never content) | 1–2 wk |

---

## 5. Risks and decisions to make before building

**Recording consent (legal).** Meeting mode transcribes other people's voices. In all-party-consent places (e.g. California and several other US states, much of the EU under GDPR) doing that without telling participants can be unlawful, even if nothing is saved. Minimum: a one-time disclosure when the user first enables meeting mode saying they're responsible for getting consent; transcripts kept in RAM only, never on disk, never in backend session memory; a clear "listening" state visible to the user. Worth a short legal read before a public release.

**Positioning.** Cluely's "cheat on everything" launch drew a lot of backlash, and interview platforms now actively detect it. For an AGPL, privacy-first project, "meeting copilot that stays out of your screen share" (sales calls, standups, support, client meetings, accessibility) is an easier story to defend than "undetectable interview helper". This affects landing copy, not the code.

**Detection is not something to fight.** As noted in 2.2, any process can see that a window is capture-excluded, and some proctoring tools check for exactly that. The scope here is "not visible in the share", not "undetectable". I'd keep it that way and document it.

**Antivirus heuristics.** An unsigned binary with a global keyboard hook, capture-excluded windows and loopback audio looks a lot like spyware to some AV engines. Expect more false positives until the installer is code-signed. Factor that into the release plan.

**Resource use.** ~1 GB RAM for Parakeet (already true when voice is on) plus continuous CPU during a call. Needs real numbers from the spike before committing to "both speakers transcribed locally" as the default.

---

## 6. Open questions for you

1. **Interaction model:** rolling transcript with an "answer now" hotkey (recommended above), or strictly record-only-while-hotkey-held?
2. **Screen context:** should the meeting answer include a screenshot by default (useful for shared docs, code or slides; adds ~300–800 ms and tokens), or only on a separate hotkey?
3. **Transcribe yourself (`ME`)?** Better answers ("you already said X") but double the CPU and the echo problem. Default on or off?
4. **Release target:** ship stealth mode on its own first (low risk, independently useful), then the copilot behind an "experimental" flag?
5. **Positioning:** general meeting copilot, or interview-focused? This mostly changes copy and default prompts.
