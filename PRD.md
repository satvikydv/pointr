# PRD: Screen-Aware AI Desktop Assistant — Windows MVP

**Codename:** ClickyWin (placeholder — rename freely)
**Author:** Satvik Yadav
**Status:** Draft v1.0
**Target platform:** Windows 10 (1903+) / Windows 11
**Target stack:** Tauri (Rust) + FastAPI (Python) + Gemini API (vision)

---

## 1. Summary

Build a lightweight, always-available desktop assistant that lives near the user's cursor. On a global hotkey press, it captures the current screen (or active monitor) along with the cursor's position, sends both to a multimodal LLM backend, and returns a contextual answer or action — rendered in a small overlay UI near the cursor.

This is a Windows-first MVP inspired by the interaction model of HeyClicky (Mac-only, YC-backed). We are not cloning their code — we're independently building an equivalent interaction pattern: hotkey → screen-aware context → LLM reasoning → overlay response, with Windows as the primary target since no direct competitor exists there yet.

---

## 2. Goals

- G1: User presses a global hotkey anywhere in Windows and gets an overlay window near their cursor within a few seconds.
- G2: The assistant receives a screenshot of the correct monitor **plus** the cursor's normalized position, and uses both as context for a Gemini vision call.
- G3: The response renders in a transparent, always-on-top overlay without stealing focus from the user's active application.
- G4: Basic "agent mode" stub exists (trigger via voice keyword or button) that queues a longer-running task via the existing FastAPI/Celery/Redis backend — full agent capability is out of scope for MVP, but the plumbing should exist.
- G5: Works reliably across common multi-monitor and DPI-scaling configurations.

## 3. Non-Goals (explicitly out of scope for MVP)

- Voice input/output (text-only for MVP; voice is a fast-follow).
- macOS support (separate PRD later if needed).
- Long-horizon autonomous agents (browser automation, multi-step task execution).
- Third-party integrations (Notion, Gmail, Calendar, Linear, etc.).
- Any persistent screen recording or continuous capture — capture happens **only** on explicit hotkey press.
- Fine-tuned grounding/pointing model — MVP uses prompt-based bounding box requests only, no custom vision model.
- Monetization / billing / auth beyond a simple API key config.

---

## 4. User Flow (MVP)

1. User installs and runs the app; it sits in the system tray, no visible window.
2. User presses global hotkey (default: `Ctrl+Alt+Space`, configurable).
3. App captures: (a) screenshot of the monitor under the cursor, (b) cursor's screen coordinates, (c) active window title (best-effort).
4. A small overlay input box appears near the cursor, focused, waiting for text input.
5. User types a question (e.g., "what does this button do?") and hits Enter.
6. Screenshot + normalized cursor coordinates + question are sent to the backend.
7. Backend calls Gemini vision API with a constructed prompt, gets back a text answer (and optionally a bounding box for a UI element to point at).
8. Overlay renders the text response. If a bounding box was returned, an animated marker is drawn at that screen location.
9. Overlay auto-dismisses on click-away, Esc, or after N seconds of inactivity (configurable, default 15s).
10. Pressing the hotkey while an overlay is open and typing "agent: <task>" queues a Celery task via the backend (stub — for MVP this can just be a task that returns "Agent mode not yet implemented" after a fake delay, to validate plumbing).

---

## 5. Architecture

```
┌─────────────────────────────┐         ┌──────────────────────────┐
│   Tauri Desktop App (Rust)  │  HTTPS  │   FastAPI Backend         │
│                              │────────▶│                          │
│  - Global hotkey listener    │         │  /analyze-screen (POST)  │
│  - Cursor position (Win32)   │         │  /agent/task (POST)      │
│  - Screen capture (WinRT)    │         │  /agent/task/{id} (GET)  │
│  - DPI reconciliation        │         │                          │
│  - Overlay window (WebView)  │◀────────│  Celery + Redis          │
│  - Marker rendering          │  JSON   │  (agent task queue)      │
└─────────────────────────────┘         │                          │
                                          │  Gemini API (vision)     │
                                          │  (Google AI SDK)         │
                                          └──────────────────────────┘
```

**Frontend/shell:** Tauri app, Rust backend for OS-level hooks, WebView (HTML/CSS/JS or a lightweight React build) for the overlay UI.

**Backend:** FastAPI, reusing existing infra patterns (Celery + Redis for task queuing, matching prior production experience with worker topology).

**LLM:** Gemini API with vision (latest available vision-capable model — confirm current recommended model at implementation time via docs, do not hardcode assumptions).

---

## 6. Detailed Component Specs

### 6.1 Global Hotkey Listener

- Use Tauri's `global-shortcut` plugin.
- Default binding: `Ctrl+Alt+Space`. Must be user-configurable via a settings file (`config.toml` or JSON in `%APPDATA%`).
- On trigger: emit an internal event `hotkey:activate` that the Rust backend listens for — this kicks off the capture pipeline.
- Must not conflict with common app shortcuts; provide a rebind UI in settings (can be minimal — a single "press new combo" capture field).

### 6.2 Cursor Position Capture

- Use Win32 `GetCursorPos()` via the `windows` Rust crate.
- Returns physical-pixel screen coordinates `(x, y)`.
- Immediately after, call `MonitorFromPoint(cursor_pos, MONITOR_DEFAULTTONEAREST)` to identify which monitor the cursor is on — this determines which monitor gets captured.
- Also fetch `GetDpiForMonitor()` for that monitor to correctly reconcile physical vs. logical pixel coordinates before normalization (see 6.4).

### 6.3 Screen Capture

- Primary method: `Windows.Graphics.Capture` (WinRT API) for the target monitor.
  - Requires Windows 10 1903+; check API availability at startup and show a clear error if unsupported.
  - First capture attempt will trigger a native OS permission/consent prompt — handle this gracefully in onboarding (show an explanatory screen before triggering it, not a bare OS dialog with no context).
- Capture only the single monitor the cursor is on, not all monitors (bandwidth + token cost + relevance).
- Fallback method (if WinRT capture fails or is unavailable): `BitBlt`-based capture via GDI. Slower and can't capture some hardware-accelerated/DRM content — acceptable degraded fallback for MVP, log a warning when this path is used.
- Output format: PNG, resized if needed to keep payload reasonable (cap long edge at ~1568px to match typical vision model input limits and control token cost — confirm current Gemini vision image size guidance in docs before finalizing this number).

### 6.4 Coordinate Normalization

- Convert cursor position from physical screen pixels to a 0.0–1.0 normalized coordinate **relative to the captured screenshot's dimensions**, not the full virtual desktop.
- Formula: `x_norm = (cursor_x - monitor_origin_x) / monitor_width_px`, same for y.
- Must account for DPI scaling reconciliation from 6.2 so the normalized point actually lands where the user's cursor visually was in the screenshot. This is the highest-risk correctness bug in the whole system — write a unit test harness that captures known cursor positions across at least two different DPI settings (100%, 150%) and asserts the normalized coordinate matches expected position within a small tolerance.

### 6.5 Payload Contract (Tauri → FastAPI)

`POST /analyze-screen`

```json
{
  "screenshot_base64": "<png bytes, base64>",
  "cursor_position": { "x_norm": 0.42, "y_norm": 0.77 },
  "screen_resolution": { "width": 2560, "height": 1440 },
  "active_window_title": "Adobe Premiere Pro",
  "query_text": "what does this button do?",
  "session_id": "uuid-v4",
  "timestamp": "2026-07-26T10:15:00Z"
}
```

Response:

```json
{
  "answer_text": "This is the 'Ripple Delete' tool...",
  "pointer_target": { "x_norm": 0.44, "y_norm": 0.79, "confidence": "medium" },
  "session_id": "uuid-v4"
}
```

`pointer_target` is optional/nullable — only present if the model was asked for and returned a bounding-box-style location.

### 6.6 Backend — `/analyze-screen` Endpoint

- FastAPI endpoint, validates payload (Pydantic model), decodes base64 image.
- Optionally composite a visible marker (small red crosshair, ~12px) onto the image at the cursor's denormalized pixel position before sending to Gemini — ship both approaches behind a config flag (`MARKER_MODE=burned_in` vs `MARKER_MODE=coords_only`) so this can be A/B tested without a redeploy.
- Construct prompt, e.g.:
  > "The user's cursor is at the marked position in this screenshot (or: at normalized position x=0.42, y=0.77 if coords-only mode). The active application is '{active_window_title}'. The user asked: '{query_text}'. Answer their question concisely, referring to what's near the cursor if relevant. If a specific UI element should be pointed at in your answer, return its approximate bounding box center in the same normalized coordinate space."
- Call Gemini API (vision-capable model, image + text content blocks per current Messages API format — check docs, do not assume prior-generation payload shape).
- Parse response; if structured pointer output requested, use a constrained JSON response format or a clearly delimited section to parse `pointer_target` reliably. Wrap parsing in try/except with a safe fallback (no pointer, text-only answer) if parsing fails — never crash the request on a malformed structured field.
- Return response per contract above.

### 6.7 Backend — Agent Task Stub

`POST /agent/task` — accepts `{ "task_description": str, "session_id": str }`, enqueues a Celery task, returns `{ "task_id": str }`.

`GET /agent/task/{task_id}` — returns `{ "status": "pending"|"running"|"done"|"failed", "result": str|null }`.

For MVP, the Celery task itself can be a stub: sleep a few seconds, return a canned "Agent mode is not yet implemented — coming soon" message. The goal of this phase is validating the queuing/polling plumbing, not building real agent capability.

### 6.8 Overlay UI

- Tauri WebView window: transparent background, always-on-top, no window decorations, no taskbar entry.
- Positioned near (not exactly on top of) the cursor position at time of hotkey press, with edge-of-screen clamping so it never renders partially off-screen.
- States: (1) input box focused and empty, (2) loading/spinner while waiting on backend, (3) response rendered as text, (4) optional animated marker drawn at `pointer_target` if present.
- Dismiss triggers: `Esc` key, click outside the overlay bounds, or configurable auto-dismiss timeout after the response is shown.
- Must not steal focus from the previously active application beyond the moment the user is actively typing into it — do not minimize or otherwise disturb the user's other windows.

---

## 7. Privacy & Data Handling

- Screenshots are captured **only** on explicit hotkey press — no continuous or background capture, no exceptions.
- Screenshots are not persisted to disk on the client by default; sent directly to the backend over HTTPS and discarded client-side after send.
- Backend should not persist raw screenshots beyond the lifetime of the request needed to call Gemini, unless the user has explicitly opted into a "history" feature (not in MVP scope — if added later, needs its own consent flow).
- Log only text summaries/metadata (query text, timestamps, session id) for debugging — never log raw image bytes in application logs.
- Settings screen must include a visible note on what is/isn't captured and a way to fully quit/disable the tray app.

---

## 8. Error Handling & Edge Cases

| Case | Expected behavior |
|---|---|
| WinRT capture API unavailable (old Windows build) | Fall back to GDI `BitBlt`; log warning; show one-time in-app notice if fallback is used |
| Captured content is DRM-protected (black frame) | Detect near-empty/black frame heuristically; return a friendly "can't see protected content" message instead of sending a useless image to the model |
| Backend unreachable / timeout | Overlay shows a clear error state with a retry button; never hang indefinitely — set a client-side timeout (e.g. 20s) |
| Multi-monitor with mixed DPI | Must use per-monitor DPI value (6.2/6.4), not a global system DPI assumption |
| Hotkey pressed while overlay already open | Second press should refresh capture (new screenshot + cursor pos), not stack a second overlay |
| Gemini API returns malformed/unparseable structured pointer data | Fall back to text-only answer, log the parse failure, do not surface a raw error to the user |
| User has no internet connection | Detect and show an explicit offline message rather than a generic timeout |

---

## 9. Tech Stack (MVP)

- **Shell:** Tauri (Rust) — chosen over Electron for smaller footprint and easier native Win32/WinRT interop.
- **Overlay UI:** Plain HTML/CSS/JS inside Tauri's WebView (keep it simple for MVP; avoid a full React build unless the team already has boilerplate ready).
- **Backend:** FastAPI (Python 3.11+).
- **Task queue:** Celery + Redis (reuse existing patterns/infra knowledge).
- **LLM:** Gemini API, vision-capable model — confirm current recommended model string against `https://ai.google.dev/gemini-api/docs` at implementation time.
- **Packaging:** Tauri's built-in Windows installer (`.msi`/`.exe` via NSIS).

---

## 10. Milestones

**M1 — Capture pipeline proof of concept**
Hotkey triggers cursor position capture + monitor screenshot capture, saved to a local file for manual inspection. No backend, no overlay yet. Validates 6.1–6.4, especially the DPI reconciliation test harness.

**M2 — Overlay shell**
Transparent always-on-top overlay appears near cursor on hotkey press with a working input box. No backend call yet — just UI shell and dismiss behavior.

**M3 — Backend integration**
Wire M1 + M2 output to a live FastAPI `/analyze-screen` endpoint calling Gemini vision. End-to-end: hotkey → screenshot → answer rendered in overlay.

**M4 — Pointer/marker rendering**
Add bounding-box request to the prompt, parse `pointer_target`, render animated marker in overlay at the correct denormalized screen position.

**M5 — Agent stub plumbing**
Add `/agent/task` + Celery stub task + polling from the overlay UI, validating the queuing pattern for future real agent work.

**M6 — Packaging & installer**
Tray app behavior, settings screen (hotkey rebind, privacy note, quit), Windows installer build.

---

## 11. Acceptance Criteria (MVP Done)

- [ ] Pressing the configured hotkey anywhere in Windows opens the overlay near the cursor within ~1 second, without stealing focus from other apps.
- [ ] The correct monitor is captured when the cursor is on a non-primary display.
- [ ] Cursor position lands within a small tolerance of its true visual position in the captured image across at least 100% and 150% DPI settings (validated by the test harness from M1).
- [ ] A typed question returns a relevant text answer from Gemini within a reasonable time budget (define target, e.g. <5s p50).
- [ ] At least one successful end-to-end pointer/marker rendering demo (model correctly identifies and points at a specific on-screen UI element).
- [ ] Agent task stub can be queued and polled to completion via the overlay.
- [ ] No screenshots persisted to disk client-side; no raw image bytes appear in backend logs.
- [ ] App runs from system tray, has a working quit action, and a minimal settings screen.

---

## 12. Open Questions for Implementation

1. What's the current recommended Gemini model string and max image dimensions for vision calls? (Check docs at implementation time — do not hardcode from this PRD.)
2. Should the marker-burned-into-image vs. coords-only prompting approach be decided via a quick internal eval before M3, or deferred to post-MVP A/B testing?
3. Auth: is a single shared API key sufficient for MVP, or does each install need its own key/account from day one?
4. Target minimum Windows version — confirmed as 1903+ for WinRT capture; confirm this doesn't exclude a meaningful chunk of target users.

---

## 13. Gap Analysis vs. HeyClicky & What's Left

**Added:** 2026-08-01. Reflects current implementation status (see `implementation-tracking.md`) against the reference product this PRD is modeled on.

### 13.1 Where this PRD intentionally diverges from HeyClicky

HeyClicky's actual shipped product is voice-first and includes a working autonomous agent mode. This PRD scoped both out for MVP deliberately (§3 Non-Goals), trading interaction fidelity for a shippable Windows skeleton first. Concretely:

| Area | HeyClicky (actual) | This PRD (MVP) | Verdict |
|---|---|---|---|
| Input | Push-to-talk: hold hotkey, speak, get spoken reply | Press hotkey, type question in overlay box | Deliberate cut — voice is fast-follow (§3) |
| Trigger | Hold-to-talk | Single press | Behavioral gap, not just a feature gap — affects hotkey plugin choice later |
| Agent mode | "heyclicky agent" actually executes the task | Stub queues Celery task, returns canned "not implemented" message | Deliberate cut (§3 Non-Goals) — plumbing only |
| Context memory | Keeps lightweight text summaries across interactions for continuity | No persistence beyond a single request unless opt-in history (§7) | Deliberate, stricter privacy default |
| Privacy model | Screen seen only on hotkey press, screenshots never stored | Same | Matches — no gap |

None of the above are bugs in the PRD — they're scope decisions. Listed here so future milestones (post-M6) know what "closing the gap with HeyClicky" actually requires: a real voice pipeline and a real agent executor, not just UI polish.

### 13.2 What's actually left to build (from current repo state)

Pulled from `implementation-tracking.md` as of 2026-07-29, plus in-flight uncommitted work as of 2026-08-01.

**M5 — Agent stub plumbing (in progress, uncommitted)**
- `backend/app/routes/agent.py`, `backend/app/models/agent.py`, `backend/app/worker/{celery_app,tasks}.py` written — Celery task queue + poll endpoints wired end to end.
- Frontend hook (`app/src/main.js`) is a manual test button ("Test Agent") with a hardcoded `session_id` and task description — needs to become a real UI entry point (e.g. an "agent:" prefix in the query box per PRD §4 step 10) before this counts as done, not just plumbing-validated.
- Polling loop in `main.js` has no timeout/max-attempt cap — will hang indefinitely if the worker container is down or a task never reaches a terminal Celery state. PRD §8 requires a client-side timeout (20s) for backend-unreachable cases; this path doesn't have one yet.

**M1 — Capture pipeline (open items)**
- Real `Windows.Graphics.Capture` (WinRT) path is stubbed (`capture_wgc` always errors) — GDI `BitBlt` is the only working path today. Means DRM/hardware-accelerated content still can't be captured, and the "log a warning on fallback" behavior is effectively always-on rather than a fallback.
- DPI reconciliation unit-test harness (100%/150%, §6.4) not written — this is the PRD-flagged highest-risk correctness bug and remains unverified.
- Disk-dump-for-inspection step from original M1 spec was dropped in favor of feeding the overlay in-memory — fine for product, but means there's no manual capture-inspection tool if a capture bug needs debugging.

**M2 — Overlay shell (design deviation, not a defect)**
- Shipped UX is fullscreen screenshot + drag-to-crop + Process/Cancel toolbar, not the PRD's small focused input box near the cursor. Functionally complete but worth a conscious decision: keep the crop-tool UX permanently, or still migrate toward the lighter PRD design before M6 polish.
- Click-outside-to-dismiss only partially implemented (Esc + Cancel button work; clicking outside the overlay bounds does not dismiss it).

**M6 — Packaging & installer (not started)**
- No tray app behavior, no settings screen (hotkey rebind UI, privacy note, quit action), no Windows installer build. All of §6.8/§9's packaging requirements and the last three MVP acceptance criteria in §11 are still open.

**Cross-cutting**
- Marker-burned-into-image mode (`MARKER_MODE=burned_in`) exists only as a config flag; the route always uses coords-in-prompt today, so the A/B in Open Question #2 hasn't actually started.
- No auth beyond implicit trust of `localhost:8000` — Open Question #3 (single shared key vs. per-install key) still unresolved and currently moot since there's no key check at all on the FastAPI side.

### 13.3 Suggested next-up order

1. Fix agent poll timeout + wire a real "agent:" trigger in the query UI (finishes M5 to the PRD's actual bar, not just plumbing).
2. Write the DPI test harness (§6.4) — highest-risk item, cheapest to defer accidentally.
3. Decide overlay UX direction (keep crop-tool vs. migrate to PRD's input-box design) before sinking more time into either.
4. M6 packaging pass once the above are stable.