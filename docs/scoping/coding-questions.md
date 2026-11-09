# Scope: Coding questions shown on screen

Status: **Phase 1 built** (not yet run on Windows), experimental branch `claude/jolly-goldberg-ncov7s`.

Built in Phase 1, with the defaults chosen:
- `Ctrl+Alt+K` (fallbacks `Ctrl+Alt+;`, `Ctrl+Alt+'`; not `Ctrl+Alt+S`, which HP laptops use for System Information), registered only during a meeting session. Captures the monitor under the cursor, cropped to the foreground window when that helps, up to 2400 px (`meeting/code.rs`, `meeting/code_util.rs`).
- Sends the screenshot, the last 2 min of transcript, the preferred language, the meeting background and the previous solution (pressing again revises) to `POST /api/meeting/code-stream`.
- **Separate coding model**, defaults `gemini-3.1-pro-preview` / `gpt-6-sol`, set in Settings → Coding questions. Spoken answers keep the everyday fast model; nothing in their path changed.
- Settings → Coding questions: language (default Python; the editor or problem on screen wins) and the coding model per provider.
- Code panel: a separate, always capture-excluded, non-focusable, click-through window docked right of the monitor under the cursor; Language line, Problem / Approach / Code / Complexity / Edge cases, highlight.js (vendored, `app/src/vendor/highlight/`) with line numbers. `Ctrl+Alt+X` hide/show, `Ctrl+Alt+PageUp/PageDown` scroll, `Ctrl+Alt+Left/Right` move the panel sideways (5% of the monitor width per press). The small meeting panel mirrors Problem and Approach.
- Answer format: optimal solution, with the brute-force idea in one Approach line when it helps.

Not built yet (Phase 2+): extra screenshots for long problems, second-monitor placement, font size and copy keys, routing code from spoken answers to the code panel, OCR assist.
Builds on meeting mode (`docs/scoping/meeting-mode.md`).

## The problem

Meeting mode answers what it **hears**. A coding question usually isn't spoken. It appears on screen:

- in a shared screen inside Zoom, Meet or Teams,
- in a coding platform in the browser (CoderPad, HackerRank, LeetCode-style editors, a shared Google Doc),
- or partly spoken ("now make it O(n)") and partly on screen.

So the copilot needs to **see** the screen, write a solution in the user's **preferred language**, and show it somewhere that has room for code. The current 480×260 meeting panel is too small for it.

## What we can reuse

| Need | Already in the codebase |
|---|---|
| Screenshot of the monitor under the cursor | `capture_now()` in `lib.rs`, `capture::screen::capture_monitor` (GDI) |
| Foreground window (to crop to the meeting/editor window) | `capture/context.rs` (`GetForegroundWindow`) |
| Resize + PNG encode for the model | `capture::resize::cap_long_edge` (1568 px), `encode_png` |
| Vision + streaming LLM call, BYOK, both providers | `services/gemini.py` / `openai_service.py` `analyze_stream(image, prompt)` |
| What was said recently (spoken constraints, follow-ups) | meeting transcript (`meeting/transcript.rs`) |
| A capture-excluded, non-activating, click-through window | the meeting panel (`stealth.rs`, `tauri.conf.json` window `meeting`) |
| Hotkeys with fallbacks when another app owns a key | `TOGGLE_CANDIDATES` pattern in `meeting/mod.rs` |
| Fenced code rendering, streamed | `renderAnswer` in `meeting.js` |

Since every Pointr window is excluded from capture, our own panels never end up in the screenshot we send. The code panel must be **always** protected, like the meeting panel, regardless of the stealth setting.

## Proposed behaviour

### Trigger

1. **"Solve from screen" hotkey** during a meeting session, with fallbacks like the meeting key: `Ctrl+Alt+S`, then `Ctrl+Alt+K`, then `Ctrl+Alt+;`.
   - Captures the screen at that moment.
   - Crops to the **foreground window** when that window is reasonably large: that's the meeting app or the coding tab. Otherwise it uses the whole monitor under the cursor. Cropping raises effective resolution where the text is; a shared screen inside Zoom is often small and blurry at full-monitor scale.
2. **Pressing it again within ~15 s** with Shift (`Ctrl+Alt+Shift+S`) **adds** a screenshot instead of starting over. This covers problems longer than one screen: scroll, press again, then answer from all the shots together.
3. **Spoken coding questions** (`Ctrl+Alt+Enter`): if the answer contains a code block, the code goes to the code panel too. The small panel keeps the sentences to say. One place for all code.

### Inputs to the model

- The screenshot(s): the problem statement, examples, constraints, and any **function stub or partial code already in the editor**.
- The last ~2 min of transcript: interviewers often add constraints out loud ("assume the input is sorted", "no extra space").
- The **preferred language** (below).
- The **previous solution** in this session, if any. A follow-up press ("now optimise it", "handle duplicates") then revises it instead of starting from scratch.
- The meeting background (brief), but **not** the uploaded documents. They're irrelevant to a coding problem, and every extra token slows the first line down.

### Preferred language

A Settings dropdown under Meeting mode: **Coding language**.

- **Options:** Python, JavaScript, TypeScript, Java, C++, C#, Go, Rust, Kotlin, Swift.
- **"Match the editor"** (default) uses the language of the editor or stub on screen when one is visible, and the preferred language otherwise.
- **The problem wins when it's explicit.** A SQL question gets SQL. "Implement this in Java" gets Java.
- **The panel header shows which language was used and why**, e.g. "Java (from the editor)". A wrong guess is then obvious.
- **Phase 2:** a hotkey that cycles the language and regenerates.

### What comes back (streamed, in this order)

```
### Problem
One line: what the model thinks is being asked. Lets the user catch a misread screen in the first second.

### Approach
2-4 short lines the user can say while starting to type: the idea, the data structure, why it works.

### Code
```<lang>
complete, idiomatic solution; uses the exact function signature / class stub from the screen when there is one;
sparse comments only where an interviewer would expect them
```

### Complexity
Time and space, one line each.

### Edge cases
3-5 short bullets: inputs to mention or test (empty, duplicates, overflow, ...).
```

The order is deliberate. The **Approach** arrives within ~2-3 s, so the user can start talking while the code is still being written.

**Decision needed:** "optimal only", or "brute force first, then optimal"? Interviewers often like the progression. That could be a second short code block, or a line in Approach.

### Where it's shown: a separate code panel

A new `code` window, separate from the small meeting panel:

| Property | Value |
|---|---|
| Size and place | ~38% of the screen width, ~85% of the height, docked to the right edge of the monitor under the cursor. A setting picks left or right, or "other monitor" when there is one (best: nothing covers the shared editor) |
| Stealth | always capture-excluded, `focusable: false`, click-through, so it never takes focus from the editor and never appears in the share |
| Look | dark, mostly opaque (code needs contrast), sections as above, **syntax highlighting** with line numbers, monospace 13-14 px |
| Keys (session only, with fallbacks) | scroll up/down, font size +/-, hide/show, dock left/right, copy code to clipboard |
| Small panel during a solve | shows "Solving… (code panel)", the Problem line and the Approach, so the user can talk without looking away |

**Syntax highlighting:** highlight.js core plus the ~12 languages above, **vendored** into `app/src/vendor/`. It's BSD-3 and roughly 100 KB. The app has no bundler and shouldn't load from a CDN at runtime. Prism is the alternative at a similar size.

**Copy to clipboard** is cheap: `write_clipboard` already exists. Many interview platforms do flag large pastes, though, so the panel shouldn't push it. **Typing the code out** via the existing `execute_type_text` isn't recommended: web editors auto-indent and auto-close brackets, which corrupts typed code.

## Backend

- **New route:** `POST /api/meeting/code-stream` (NDJSON, same event shape as `answer-stream`).
- **Request body:**
  - `screenshots: [base64]` (1-4 images)
  - `transcript` (last ~2 min)
  - `language` (`"auto"` or a name), `preferred_language`
  - `previous_solution` (optional)
  - `brief`, plus the BYOK fields
- **`analyze_stream` takes a list of images** instead of one: Gemini `contents=[prompt, *parts]`, OpenAI several `input_image` items. Each existing caller passes `[image]`.
- **Prompt rules:**
  - read the stub and the current editor contents and keep that signature
  - prefer the clearest correct solution at the expected complexity
  - never claim information is missing: if something's cut off, assume the standard version of the problem and say so in the Problem line
  - output the sections exactly as above
- **Stateless**, like the rest of meeting mode.

## Latency

| Step | Estimate |
|---|---|
| Capture + crop + PNG encode | 100-300 ms (measured path today) |
| Upload (1 image, ~0.5-1.5 MB) | 100-400 ms |
| Time to first token (vision, Flash/mini-class) | 1-3 s (the 5.1 s seen so far needs looking at, see below) |
| Problem + Approach readable | ~2-4 s after the press |
| Full code (40-60 lines) | ~5-12 s after the press |

**The 5.1 s first-token time already measured** for text answers needs fixing first. It's most likely the model's "thinking" step. A per-route setting to turn thinking off or down for meeting answers would help both features. Hard algorithm problems are where thinking pays off, though, so this feature may want its own **"Coding model"** setting: a stronger, slower model just for solves, stated plainly in Settings.

## Risks and limits

- **Misreads.** Small or blurry shared-screen text, or a problem cut off below the fold. Mitigated by the crop, by "add another screenshot", and by the Problem line showing what was understood. Local OCR as a second signal (Windows.Media.Ocr, no download) is a Phase 3 option.
- **Screen sharing.** The panel is excluded from capture, but the user's eyes and pauses aren't. Docking on a second monitor is the most natural setup.
- **Platform detection.** Coding platforms commonly log paste events, tab/window blur and typing cadence. Nothing here steals focus or pastes by itself, but copy-and-paste by the user is visible to them.
- **Cost.** A vision request with one or two screenshots is a few thousand input tokens per solve, on the user's own key.

## Phases

| Phase | Scope | Estimate |
|---|---|---|
| 1 | "Solve from screen" hotkey (monitor or foreground-window crop), language setting with "match the editor", `/code-stream` with multi-image services, code panel with highlighting and scroll/hide keys, Problem/Approach mirrored in the small panel, follow-up press revises the previous solution | 3-5 days |
| 2 | "Add screenshot" multi-shot, spoken-question code routed to the code panel, dock side / other-monitor setting, font size keys, copy key, language-cycle key, "Coding model" setting | 3-4 days |
| 3 | OCR assist, brute-force-then-optimal variants, per-section collapse | later |

## Open questions

1. Hotkey: is `Ctrl+Alt+S` (with fallbacks) fine?
2. Default language when the editor doesn't say: which one?
3. Panel placement: right edge of the current monitor, or the other monitor when there is one?
4. "Optimal only", or "brute force, then optimal"?
5. Separate "Coding model" setting, or the same model as everything else?
