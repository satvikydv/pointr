const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const container = document.getElementById('overlay-container');
const img = document.getElementById('screenshot-img');
const selectionBox = document.getElementById('selection-box');
const selectionLabel = document.getElementById('selection-label');
const toolbar = document.getElementById('toolbar');
const btnCancel = document.getElementById('btn-cancel');
const btnSubmit = document.getElementById('btn-submit');
const queryInput = document.getElementById('query-input');
const loadingIndicator = document.getElementById('loading-indicator');
const loadingLabel = document.getElementById('loading-label');
const directQueryBox = document.getElementById('direct-query-box');
const directQueryInput = document.getElementById('direct-query-input');
const errorToast = document.getElementById('error-toast');
const errorMessage = document.getElementById('error-message');
const errorClose = document.getElementById('error-close');

let errorTimer = null;

// Unlike the alert() this replaced, the toast doesn't block — so whatever
// should happen once the user's done with it (typically hiding the window
// again) has to run from here, on close or timeout, not right after the
// call that raised it.
function showError(message, onDismissed) {
    if (errorTimer) clearTimeout(errorTimer);
    errorMessage.textContent = message;
    errorToast.classList.remove('hidden');
    const dismiss = () => {
        errorToast.classList.add('hidden');
        if (onDismissed) onDismissed();
    };
    errorClose.onclick = () => {
        clearTimeout(errorTimer);
        dismiss();
    };
    errorTimer = setTimeout(dismiss, 6000);
}

// Builds (or returns the existing) answer bubble: a small "POINTR" header
// plus a body span the streaming listener appends into. Kept as one helper
// so the stream-chunk listener and renderResponse never disagree on shape.
function getOrCreateTooltip() {
    let tooltip = document.getElementById('answer-tooltip');
    if (tooltip) return tooltip;

    tooltip = document.createElement('div');
    tooltip.id = 'answer-tooltip';
    tooltip.className = 'answer-tooltip';
    tooltip.innerHTML = `
        <div class="answer-header">
            <div class="avatar">
                <div class="dot"></div>
                <div class="speaking-orb">
                    <div class="orb-bloom"></div>
                    <div class="orb-core">
                        <div class="orb-swirl1"></div>
                        <div class="orb-swirl2"></div>
                        <div class="orb-nebula"></div>
                        <div class="orb-particle p1"></div>
                        <div class="orb-particle p2"></div>
                        <div class="orb-particle p3"></div>
                        <div class="orb-highlight"></div>
                    </div>
                </div>
            </div>
            <div class="label">Pointr</div>
        </div>
        <div class="answer-body"><span id="answer-text"></span><span id="answer-caret" class="answer-caret"></span></div>
    `;
    container.appendChild(tooltip);
    return tooltip;
}

let speakingTimer = null;
let ttsEndedUnlisten = null;
let speakingOnEnded = null;

// Swaps the header's plain dot for the animated "speaking" orb, synced to
// Rust's real `tts-ended` event (emitted from MediaPlayer's MediaEnded) —
// registered *before* speak_text is invoked so there's no race with a very
// short utterance finishing before the listener attaches. The word-count
// timer is now just a safety ceiling in case the event is ever missed
// (playback error swallowed, event lost), generous enough to never fire
// before real narration of that length would plausibly finish.
//
// `header` must be passed explicitly by the caller, not looked up here via a
// blind `.answer-header` selector — the normal answer tooltip's header and
// the storyboard bar's header both alias the same class and are BOTH always
// present in the DOM at once (the storyboard bar is just hidden via a CSS
// class, not removed), and the storyboard bar happens to appear first in
// document order. A blind selector silently toggled `.speaking` on the
// hidden storyboard header for every normal narrated answer — no visible
// effect, since it's not the one on screen — while storyboard mode's own
// header happened to be right by coincidence. Real bug, found live.
async function startSpeakingIndicator(text, onEnded, header) {
    stopSpeakingIndicator();
    if (!header) return;
    header.classList.add('speaking');
    speakingOnEnded = onEnded || null;

    try {
        ttsEndedUnlisten = await listen('tts-ended', () => finishSpeakingIndicator());
    } catch (e) {
        console.error('Failed to listen for tts-ended:', e);
    }

    const wordCount = text ? text.split(/\s+/).length : 0;
    const fallbackMs = Math.max(4000, (wordCount / 2.5) * 1000 * 2.5);
    speakingTimer = setTimeout(() => finishSpeakingIndicator(), fallbackMs);
}

// Natural end (real event or fallback timeout) — runs the caller's onEnded
// callback exactly once, unlike stopSpeakingIndicator() which is a hard stop
// (dismiss) with no callback.
function finishSpeakingIndicator() {
    const cb = speakingOnEnded;
    stopSpeakingIndicator();
    if (cb) cb();
}

function stopSpeakingIndicator() {
    if (speakingTimer) {
        clearTimeout(speakingTimer);
        speakingTimer = null;
    }
    if (ttsEndedUnlisten) {
        ttsEndedUnlisten();
        ttsEndedUnlisten = null;
    }
    speakingOnEnded = null;
    // Clears from whichever header(s) actually have it — both the tooltip's
    // and the storyboard bar's answer-header alias the same class, and only
    // one is ever actually marked speaking in practice, but this doesn't
    // need to guess which; querySelectorAll finds it regardless of caller.
    document.querySelectorAll('.answer-header.speaking').forEach((h) => h.classList.remove('speaking'));
}

let isDrawing = false;
let startX = 0;
let startY = 0;
let currentRect = null;
let closeTimer = null;
// 'region' = manual drag-to-crop flow (secondary hotkey, Ctrl+Alt+Shift+Space)
// 'direct' = single-press capture + ask-a-question flow (primary hotkey, Ctrl+Alt+Space)
let mode = 'region';

// Shared across both flows: tags each in-flight backend request so a stale
// one (superseded by a newer press/submission, or outlived by a manual
// dismiss) can be told apart from the current one and ignored instead of
// resurrecting the overlay with old content (PRD §8: "second press should
// refresh capture... not stack a second overlay").
let activeRequestId = 0;

// Secondary hotkey: manual region-select flow. Shows the full screenshot and
// waits for the user to drag a crop box before doing anything.
listen('show-overlay', async (event) => {
    if (closeTimer) clearTimeout(closeTimer);
    mode = 'region';
    activeRequestId++;

    const appWindow = Window.getCurrent();
    await appWindow.setIgnoreCursorEvents(false);

    img.src = event.payload;
    resetSelection();
});

// Escape is normally just a DOM keydown listener (see bottom of file), but
// that depends on the webview actually holding OS keyboard focus — which is
// unreliable while the window is click-through (setIgnoreCursorEvents(true),
// used throughout answer display). These wrap a Rust-side global shortcut
// that works regardless of focus; registered only while an answer is
// showing, so it doesn't swallow Escape for other apps the rest of the time.
async function enableEscapeDismiss() {
    try {
        await invoke('enable_escape_dismiss');
    } catch (e) {
        console.warn('enable_escape_dismiss failed:', e);
    }
}

async function disableEscapeDismiss() {
    try {
        await invoke('disable_escape_dismiss');
    } catch (e) {
        console.warn('disable_escape_dismiss failed:', e);
    }
}

listen('dismiss-overlay', async () => {
    await dismissOverlay();
    resetSelection();
});

// Live answer streaming: the Rust command forwards each chunk from the
// backend's streaming endpoint here, tagged with the request_id it was
// called with. Only chunks matching the current request are applied — a
// superseded request's trailing chunks are dropped, same idea as the
// requestId guard on the final response.
listen('analyze-stream-chunk', (event) => {
    const { request_id, text } = event.payload;
    if (String(activeRequestId) !== request_id) return;

    loadingIndicator.classList.add('hidden');
    getOrCreateTooltip();
    document.getElementById('answer-text').textContent += text;
});

// Primary hotkey: fires immediately on hotkey press. Rust has already
// captured the screen, burned a marker in at the cursor position, and
// stashed it — this just opens a small input for the user's question. The
// actual backend call only fires once they submit (see runDirectAnalysis).
listen('show-overlay-direct', async () => {
    if (closeTimer) clearTimeout(closeTimer);
    mode = 'direct';
    activeRequestId++; // invalidate anything still in flight from before

    img.style.display = 'none';
    selectionBox.style.display = 'none';
    selectionLabel.classList.add('hidden');
    toolbar.classList.add('hidden');
    currentRect = null;
    loadingIndicator.classList.add('hidden');
    const oldTooltip = document.getElementById('answer-tooltip');
    if (oldTooltip) oldTooltip.remove();
    const oldMarker = document.getElementById('pointer-marker');
    if (oldMarker) oldMarker.remove();

    const appWindow = Window.getCurrent();

    // Question step is interactive — unlike the click-through answer-display
    // phase, the user needs to actually type here, so the window must accept
    // keyboard/mouse input.
    await appWindow.setIgnoreCursorEvents(false);
    await appWindow.show();
    await appWindow.setFocus();

    directQueryInput.value = '';
    directQueryBox.classList.remove('hidden');
    directQueryInput.focus();
});

directQueryInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
        e.preventDefault();
        const text = directQueryInput.value.trim();
        directQueryBox.classList.add('hidden');
        runDirectAnalysis(text);
    }
    // Escape falls through to the document-level keydown handler below —
    // the window is interactive at this point, so it's reliably delivered.
});

async function runDirectAnalysis(queryText) {
    const requestId = ++activeRequestId;
    const appWindow = Window.getCurrent();

    const agentMatch = queryText.trim().match(/^agent:\s*(.*)$/i);
    const explainMatch = queryText.trim().match(/^explain:\s*(.*)$/i);

    loadingLabel.textContent = agentMatch ? 'Queueing…' : (explainMatch ? 'Preparing…' : 'Thinking…');
    loadingIndicator.classList.remove('hidden');
    await appWindow.setIgnoreCursorEvents(true);
    await enableEscapeDismiss();

    try {
        if (explainMatch) {
            const topic = explainMatch[1] || queryText;
            const storyboard = await invoke('process_explain', { topic });
            console.log('[explain] storyboard steps:', JSON.stringify(storyboard.steps, null, 2));

            if (requestId !== activeRequestId) return; // superseded by a later press

            loadingIndicator.classList.add('hidden');
            await appWindow.show();
            await appWindow.setAlwaysOnTop(true);
            await appWindow.setFocus();

            await playStoryboard(storyboard.steps, requestId);
            return;
        }

        const agentTaskDescription = agentMatch ? (agentMatch[1] || "Analyze this for agent actions") : null;
        const response = agentMatch
            ? await runAgentTask(agentTaskDescription)
            : await invoke('process_direct', {
                query: queryText || null,
                requestId: String(requestId)
            });

        if (requestId !== activeRequestId) return; // superseded by a later press

        loadingIndicator.classList.add('hidden');
        // Re-assert show()/alwaysOnTop, not just setFocus(). alwaysOnTop is
        // set once at window creation — if any other topmost window (a
        // fullscreen video, a PiP player, DevTools) has since grabbed
        // topmost status, our window can end up genuinely behind it in
        // z-order even though Tauri still considers it "shown". Forcing it
        // here re-asserts frontmost at the exact moment we display the answer.
        await appWindow.show();
        await appWindow.setAlwaysOnTop(true);
        await appWindow.setFocus();

        if (response.multi_step_plan) {
            await runMultiStepLoop(agentTaskDescription, response.multi_step_plan);
            return;
        }

        await confirmProposedActionIfAny(response.proposed_action);
        if (requestId !== activeRequestId) return; // could've been superseded during the confirm wait

        const rect = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
        renderResponse(response, rect);
    } catch (error) {
        if (requestId !== activeRequestId) return;

        console.error("Error in direct analysis:", error);
        loadingIndicator.classList.add('hidden');
        // Window may still be click-through from the setIgnoreCursorEvents(true)
        // above — flip it off *before* showing the toast so it's dismissable.
        await appWindow.show();
        await appWindow.setFocus();
        await appWindow.setIgnoreCursorEvents(false);
        await disableEscapeDismiss();
        showError(`${error}`, async () => {
            await appWindow.hide();
            await appWindow.setIgnoreCursorEvents(false);
        });
    }
}

// "explain: <topic>" storyboard playback — sequentially shows each step's
// narration + optional annotation in a dedicated bottom caption bar (not the
// normal answer bubble), waiting for that step's narration to actually
// finish speaking (real tts-ended event, or the fallback estimate — see
// startSpeakingIndicator) before advancing, rather than a fixed delay per
// step. `requestId` guards against a dismiss/new-request superseding this
// mid-playback (checked between every step, not just at the start).
async function playStoryboard(steps, requestId) {
    const bar = document.getElementById('storyboard-bar');
    const progress = document.getElementById('storyboard-progress');
    const caption = document.getElementById('storyboard-caption');
    const hint = document.getElementById('storyboard-hint');

    progress.innerHTML = '';
    const dotEls = steps.map(() => {
        const dot = document.createElement('div');
        dot.className = 'storyboard-progress-dot';
        progress.appendChild(dot);
        return dot;
    });
    const setActiveDot = (activeIdx) => {
        dotEls.forEach((dot, idx) => dot.classList.toggle('active', idx === activeIdx));
    };

    const endStoryboardUi = () => {
        clearTimeout(hintTimer);
        hint.classList.add('hidden');
        bar.classList.add('hidden');
        clearStoryboardShapes();
    };

    bar.classList.remove('hidden');
    hint.classList.remove('hidden');
    let hintTimer = setTimeout(() => hint.classList.add('hidden'), 4200);

    for (let i = 0; i < steps.length; i++) {
        if (requestId !== activeRequestId) { endStoryboardUi(); return; }
        const step = steps[i];

        setActiveDot(i);
        caption.textContent = step.narration;
        renderStoryboardShape(step);

        await speakStepAndWait(step.narration);
    }

    clearTimeout(hintTimer);
    hint.classList.add('hidden');
    if (requestId !== activeRequestId) { bar.classList.add('hidden'); clearStoryboardShapes(); return; }
    clearStoryboardShapes();
    bar.classList.add('hidden');
    if (closeTimer) clearTimeout(closeTimer);
    closeTimer = setTimeout(() => dismissOverlay(), 5000);
}

function clearStoryboardShapes() {
    const oldMarker = document.getElementById('pointer-marker');
    if (oldMarker) oldMarker.remove();
    const oldBox = document.getElementById('storyboard-box');
    if (oldBox) oldBox.remove();
    const oldLine = document.getElementById('storyboard-line');
    if (oldLine) oldLine.remove();
    const oldHead = document.getElementById('storyboard-line-head');
    if (oldHead) oldHead.remove();
}

// Renders whichever single shape a storyboard step carries. "point" reuses
// the same red (#ff5470) marker the normal single-answer flow draws. "box"
// and "line" are blue (#5B8CFF, the UI-chrome accent — point stays a distinct
// "reference" red, box/line read as "here's the relevant area") SVG elements
// in #storyboard-svg, each with a draw-on animation: box traces its own
// perimeter via stroke-dashoffset, line extends from start to end the same
// way with a computed arrowhead (perpendicular-offset triangle, not an SVG
// <marker>) that only fades in once the line is mostly drawn.
function renderStoryboardShape(step) {
    clearStoryboardShapes();
    if (!step.shape) return;

    const x1 = clampUnit(step.x_norm) * window.innerWidth;
    const y1 = clampUnit(step.y_norm) * window.innerHeight;

    if (step.shape === 'point') {
        const marker = document.createElement('div');
        marker.id = 'pointer-marker';
        marker.className = 'pointer-marker';
        marker.style.left = x1 + 'px';
        marker.style.top = y1 + 'px';
        container.appendChild(marker);
        return;
    }

    if (step.x2_norm == null || step.y2_norm == null) return;
    const x2 = clampUnit(step.x2_norm) * window.innerWidth;
    const y2 = clampUnit(step.y2_norm) * window.innerHeight;
    const svg = document.getElementById('storyboard-svg');
    const SVG_NS = 'http://www.w3.org/2000/svg';

    if (step.shape === 'box') {
        const bx = Math.min(x1, x2);
        const by = Math.min(y1, y2);
        const bw = Math.abs(x2 - x1);
        const bh = Math.abs(y2 - y1);
        const perimeter = 2 * (bw + bh);

        const rect = document.createElementNS(SVG_NS, 'rect');
        rect.id = 'storyboard-box';
        rect.setAttribute('x', bx);
        rect.setAttribute('y', by);
        rect.setAttribute('width', bw);
        rect.setAttribute('height', bh);
        rect.setAttribute('rx', 5);
        rect.setAttribute('fill', 'rgba(91,140,255,0.03)');
        rect.setAttribute('stroke', '#5B8CFF');
        rect.setAttribute('stroke-width', 2);
        rect.setAttribute('stroke-dasharray', String(perimeter));
        rect.setAttribute('stroke-dashoffset', String(perimeter));
        svg.appendChild(rect);

        rect.getBoundingClientRect(); // force layout before animating
        rect.style.transition = 'stroke-dashoffset 0.5s ease, fill 0.5s ease';
        requestAnimationFrame(() => {
            rect.setAttribute('stroke-dashoffset', '0');
            rect.setAttribute('fill', 'rgba(91,140,255,0.12)');
        });
    } else if (step.shape === 'line') {
        const dx = x2 - x1;
        const dy = y2 - y1;
        const len = Math.hypot(dx, dy) || 1;
        const ux = dx / len, uy = dy / len;
        const px = -uy, py = ux; // perpendicular unit vector, for the arrowhead's width
        const headLen = 11, headWidth = 4.5;
        const backX = x2 - ux * headLen;
        const backY = y2 - uy * headLen;

        const path = document.createElementNS(SVG_NS, 'path');
        path.id = 'storyboard-line';
        path.setAttribute('d', `M ${x1} ${y1} L ${x2} ${y2}`);
        path.setAttribute('stroke', '#5B8CFF');
        path.setAttribute('stroke-width', 2.5);
        path.setAttribute('fill', 'none');
        path.setAttribute('stroke-linecap', 'round');
        path.setAttribute('stroke-dasharray', String(len));
        path.setAttribute('stroke-dashoffset', String(len));
        svg.appendChild(path);

        const head = document.createElementNS(SVG_NS, 'polygon');
        head.id = 'storyboard-line-head';
        const points = `${x2},${y2} ${(backX + px * headWidth).toFixed(1)},${(backY + py * headWidth).toFixed(1)} ${(backX - px * headWidth).toFixed(1)},${(backY - py * headWidth).toFixed(1)}`;
        head.setAttribute('points', points);
        head.setAttribute('fill', '#5B8CFF');
        head.style.opacity = '0';
        svg.appendChild(head);

        path.getBoundingClientRect();
        path.style.transition = 'stroke-dashoffset 0.4s ease';
        head.style.transition = 'opacity 0.15s ease';
        requestAnimationFrame(() => {
            path.setAttribute('stroke-dashoffset', '0');
            setTimeout(() => { head.style.opacity = '1'; }, 280); // appear once the line's nearly drawn, not before
        });
    }
}

// Resolves once this step's narration has finished (or a fixed pause, if
// narration is off/fails) — the thing playStoryboard awaits between steps.
function speakStepAndWait(text) {
    return new Promise((resolve) => {
        invoke('get_speech_enabled')
            .then(async (enabled) => {
                if (!enabled) {
                    setTimeout(resolve, 1800);
                    return;
                }
                const header = document.querySelector('#storyboard-bar .answer-header');
                await startSpeakingIndicator(text, resolve, header);
                invoke('speak_text', { text, voiceId: null }).catch(() => resolve());
            })
            .catch(() => setTimeout(resolve, 1800));
    });
}

function resetSelection() {
    img.style.display = 'block';
    selectionBox.style.display = 'none';
    selectionLabel.classList.add('hidden');
    toolbar.classList.add('hidden');
    currentRect = null;
    queryInput.value = '';
    directQueryBox.classList.add('hidden');
    directQueryInput.value = '';
}

container.addEventListener('mousedown', (e) => {
    // Region-select drag only applies in the secondary (manual crop) flow.
    if (mode !== 'region') return;
    // If clicking on toolbar, ignore
    if (e.target.closest('#toolbar')) return;

    isDrawing = true;
    startX = e.clientX;
    startY = e.clientY;

    selectionBox.style.left = startX + 'px';
    selectionBox.style.top = startY + 'px';
    selectionBox.style.width = '0px';
    selectionBox.style.height = '0px';
    selectionBox.style.display = 'block';
    toolbar.classList.add('hidden');
    selectionLabel.classList.remove('hidden');
});

container.addEventListener('mousemove', (e) => {
    if (!isDrawing) return;

    const currentX = e.clientX;
    const currentY = e.clientY;

    const width = Math.abs(currentX - startX);
    const height = Math.abs(currentY - startY);
    const left = Math.min(startX, currentX);
    const top = Math.min(startY, currentY);

    selectionBox.style.width = width + 'px';
    selectionBox.style.height = height + 'px';
    selectionBox.style.left = left + 'px';
    selectionBox.style.top = top + 'px';

    selectionLabel.textContent = `${Math.round(width)} × ${Math.round(height)}`;
    selectionLabel.style.left = left + 'px';
    selectionLabel.style.top = (top - 20) + 'px';
});

container.addEventListener('mouseup', (e) => {
    if (!isDrawing) return;
    isDrawing = false;

    // If the user just clicked without dragging, make a default 100x100 box
    if (selectionBox.offsetWidth <= 10 || selectionBox.offsetHeight <= 10) {
        const size = 100;
        selectionBox.style.width = size + 'px';
        selectionBox.style.height = size + 'px';
        selectionBox.style.left = (startX - size/2) + 'px';
        selectionBox.style.top = (startY - size/2) + 'px';
    }

    toolbar.classList.remove('hidden');
    selectionLabel.classList.add('hidden');

    // Position toolbar just below the selection box
    const boxRect = selectionBox.getBoundingClientRect();
    toolbar.style.bottom = 'auto';
    toolbar.style.right = 'auto';
    toolbar.style.left = boxRect.left + 'px';
    toolbar.style.top = (boxRect.bottom + 10) + 'px';

    currentRect = {
        x: parseInt(selectionBox.style.left),
        y: parseInt(selectionBox.style.top),
        width: selectionBox.offsetWidth,
        height: selectionBox.offsetHeight
    };

    queryInput.value = '';
    queryInput.focus();
});

queryInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !btnSubmit.disabled) {
        e.preventDefault();
        btnSubmit.click();
    }
});

btnCancel.addEventListener('click', async () => {
    const appWindow = Window.getCurrent();
    await appWindow.hide();
    resetSelection();
});

// Poll cap: originally 20 attempts * 1s = 20s, matching PRD §8's client-side
// timeout for a backend that never reaches a terminal state (worker down,
// stuck task). That was calibrated when every agent task was a single
// Gemini call — the filesystem/GitHub/multi-step phases each add real extra
// round trips (an MCP handshake, a second and sometimes third sequential
// Gemini call) that can genuinely take longer than 20s, especially GitHub's
// remote server over the network. User-reported: task succeeded (visible in
// the worker logs) but the UI had already shown a timeout error by then.
// 60s still catches a truly stuck/dead worker, just doesn't fire early on
// a slower-but-healthy multi-phase task.
const AGENT_POLL_MAX_ATTEMPTS = 60;
const AGENT_POLL_INTERVAL_MS = 1000;

async function runAgentTask(taskDescription) {
    const sessionId = crypto.randomUUID();

    let clipboardText = "";
    try {
        clipboardText = await invoke('read_clipboard');
        console.log(`[agent] clipboard read: ${clipboardText.length} chars, preview="${clipboardText.slice(0, 40)}"`);
    } catch (e) {
        console.error("[agent] Failed to read clipboard:", e);
    }

    // Without this, agent tasks were text-only and had nothing to look at —
    // "reply to the message on screen" fell back to clipboard content as the
    // only available context, since it was the only thing there was.
    let screenshotBase64 = "";
    try {
        screenshotBase64 = await invoke('get_current_screenshot_base64');
    } catch (e) {
        console.error("[agent] Failed to read current screenshot:", e);
    }

    // Decrypted (DPAPI) fresh per request, same as clipboard/screenshot —
    // never persisted anywhere beyond this one request; empty string if
    // GitHub isn't connected in Settings, not an error.
    let githubToken = "";
    try {
        githubToken = await invoke('get_github_token_for_request');
    } catch (e) {
        console.error("[agent] Failed to read GitHub token:", e);
    }

    const postRes = await fetch("http://localhost:8000/api/agent/task", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
            task_description: taskDescription,
            session_id: sessionId,
            clipboard_text: clipboardText,
            screenshot_base64: screenshotBase64,
            github_token: githubToken
        })
    });
    const { task_id } = await postRes.json();

    for (let attempt = 0; attempt < AGENT_POLL_MAX_ATTEMPTS; attempt++) {
        await new Promise(r => setTimeout(r, AGENT_POLL_INTERVAL_MS));

        const getRes = await fetch(`http://localhost:8000/api/agent/task/${task_id}`);
        const statusData = await getRes.json();

        if (statusData.status === "SUCCESS") {
            const clipboardWrite = statusData.result.clipboard_write;
            if (typeof clipboardWrite === 'string') {
                try {
                    await invoke('write_clipboard', { text: clipboardWrite });
                } catch (e) {
                    console.error("Failed to write clipboard:", e);
                }
            }
            return {
                answer_text: statusData.result.result,
                pointer_target: statusData.result.pointer_target,
                proposed_action: statusData.result.proposed_action || null,
                multi_step_plan: statusData.result.multi_step_plan || null
            };
        } else if (statusData.status === "FAILURE") {
            throw new Error("Task failed: " + statusData.result);
        }
    }

    throw new Error(`Agent task timed out after ${AGENT_POLL_MAX_ATTEMPTS * AGENT_POLL_INTERVAL_MS / 1000}s`);
}

// Gate before any agent-proposed desktop action (type_text/open_app)
// actually runs — shows the model's plain-language description and waits
// for a real keypress. Window goes interactive for this (Enter/Esc via a
// plain DOM listener), same as direct-query-box, since every other overlay
// state is click-through and a clickable Confirm/Cancel button would have
// the exact same "can never be clicked" problem already hit elsewhere.
// No-op if there's no proposed action (the common case).
async function confirmProposedActionIfAny(action) {
    if (!action) return;

    try {
        const enabled = await invoke('get_os_actions_enabled');
        if (!enabled) return; // OS actions off in Settings — skip silently, answer_text still shows normally
    } catch (e) {
        console.error('Failed to check os_actions_enabled:', e);
        return; // fail closed — no confirm prompt, no action, if the setting can't be read
    }

    const appWindow = Window.getCurrent();
    const box = document.getElementById('action-confirm-box');
    const typeSection = document.getElementById('action-confirm-type');
    const openSection = document.getElementById('action-confirm-open');
    const runningPill = document.getElementById('action-running-pill');

    let target = '';
    if (action.action_type === 'type_text') {
        try {
            target = await invoke('get_active_window_title');
        } catch (e) {
            console.error('Failed to read active window title:', e);
        }
        document.getElementById('action-confirm-type-title').textContent = target ? `Type into ${target}` : 'Type into the focused field';
        typeSection.classList.remove('hidden');
        openSection.classList.add('hidden');
    } else if (action.action_type === 'open_app') {
        document.getElementById('action-confirm-open-title').textContent = `Open ${action.app_name}`;
        openSection.classList.remove('hidden');
        typeSection.classList.add('hidden');
    } else {
        return; // unrecognized action type — nothing sensible to confirm
    }

    await appWindow.setIgnoreCursorEvents(false);
    await appWindow.setFocus();
    box.classList.remove('hidden');

    // The global Escape-dismiss shortcut (enableEscapeDismiss, registered
    // earlier in the flow) is still live at this point — left alone, an Esc
    // press here could trigger dismissOverlay() (hides the window) *without*
    // ever resolving this promise, orphaning the keydown listener below with
    // its "resolve" still pending. A later, totally unrelated Enter press
    // would then resolve it as a confirm and run the stale action for real.
    // Disabling it for the duration means Escape here can only ever be
    // handled by the local listener below.
    await disableEscapeDismiss();

    const confirmed = await new Promise((resolve) => {
        const onKey = (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                document.removeEventListener('keydown', onKey);
                resolve(true);
            } else if (e.key === 'Escape') {
                e.preventDefault();
                document.removeEventListener('keydown', onKey);
                resolve(false);
            }
        };
        document.addEventListener('keydown', onKey);
    });

    if (!confirmed) {
        // Esc here cancels the action AND closes the whole overlay — same
        // meaning Esc has everywhere else in the app, rather than skipping
        // just the action and falling through to show the answer anyway.
        // dismissOverlay() handles hiding action-confirm-box, restoring
        // click-through, and disabling the escape shortcut itself, so there's
        // nothing left to re-enable/restore here.
        await dismissOverlay();
        return;
    }

    await enableEscapeDismiss();

    box.classList.add('hidden');
    // Restore click-through — every other answer-display state runs this
    // way, so the normal renderResponse() flow that follows stays consistent.
    await appWindow.setIgnoreCursorEvents(true);

    document.getElementById('action-running-text').textContent =
        action.action_type === 'type_text' ? `Typing into ${target || 'the focused field'}…` : `Opening ${action.app_name}…`;
    runningPill.classList.remove('hidden');

    try {
        if (action.action_type === 'type_text') {
            await invoke('execute_type_text', { text: action.text });
        } else if (action.action_type === 'open_app') {
            await invoke('execute_open_app', { appName: action.app_name });
        }
    } catch (e) {
        console.error('Action execution failed:', e);
        showError(`Action failed: ${e}`);
    } finally {
        runningPill.classList.add('hidden');
    }
}

const MULTI_STEP_MAX_STEPS = 12;
const MULTI_STEP_STEP_DELAY_MS = 900; // let the UI settle after an action before the next screenshot

// Gate before a multi-step automation sequence runs — shows the model's
// rough plan once (not per-step, tedious past 2-3 steps) and waits for a
// real keypress, same confirm pattern as confirmProposedActionIfAny. Once
// confirmed, steps run automatically: fresh screenshot -> ask the backend
// for just the next action -> execute it -> repeat, re-grounding off the
// real screen each time rather than trusting the upfront plan's guesses.
// No-op if there's no plan (the common case).
async function runMultiStepLoop(taskDescription, plan) {
    if (!plan || !plan.length) return;

    try {
        const enabled = await invoke('get_os_actions_enabled');
        if (!enabled) return; // OS actions off — skip silently, answer_text still shows normally
    } catch (e) {
        console.error('Failed to check os_actions_enabled:', e);
        return; // fail closed
    }

    const appWindow = Window.getCurrent();
    const box = document.getElementById('multistep-confirm-box');
    const title = document.getElementById('multistep-confirm-title');
    const planList = document.getElementById('multistep-confirm-plan');
    const progressPill = document.getElementById('multistep-progress-pill');
    const progressText = document.getElementById('multistep-progress-text');

    title.textContent = `I'll do this in ${plan.length} steps:`;
    planList.innerHTML = '';
    plan.forEach((step) => {
        const li = document.createElement('li');
        li.textContent = step;
        planList.appendChild(li);
    });

    await appWindow.setIgnoreCursorEvents(false);
    await appWindow.setFocus();
    box.classList.remove('hidden');

    // Same race avoided as confirmProposedActionIfAny: disable the global
    // Escape shortcut for the duration of this local keydown-based confirm,
    // so only this listener can ever resolve it.
    await disableEscapeDismiss();

    const confirmed = await new Promise((resolve) => {
        const onKey = (e) => {
            if (e.key === 'Enter') {
                e.preventDefault();
                document.removeEventListener('keydown', onKey);
                resolve(true);
            } else if (e.key === 'Escape') {
                e.preventDefault();
                document.removeEventListener('keydown', onKey);
                resolve(false);
            }
        };
        document.addEventListener('keydown', onKey);
    });

    box.classList.add('hidden');

    if (!confirmed) {
        await dismissOverlay();
        return;
    }

    // Running phase: window goes click-through like every other answer
    // state, and Escape routes through the global shortcut again (a
    // click-through window can't reliably keep DOM keyboard focus — the
    // same reason that shortcut exists at all). dismissOverlay() bumps
    // activeRequestId, which this loop checks between every step to abort.
    await appWindow.setIgnoreCursorEvents(true);
    await enableEscapeDismiss();
    const requestId = activeRequestId;

    progressPill.classList.remove('hidden');
    const completedSteps = [];
    let finalAnswer = null;

    for (let i = 0; i < MULTI_STEP_MAX_STEPS; i++) {
        if (requestId !== activeRequestId) return; // aborted

        progressText.textContent = `Step ${i + 1}: deciding…`;

        let screenshotBase64;
        try {
            screenshotBase64 = await invoke('capture_fresh_screenshot');
        } catch (e) {
            console.error('Failed to capture screenshot for step:', e);
            finalAnswer = 'Lost track of the screen mid-task — stopping here.';
            break;
        }

        let step;
        try {
            const res = await fetch("http://localhost:8000/api/agent/step", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({
                    task_description: taskDescription,
                    plan,
                    completed_steps: completedSteps,
                    screenshot_base64: screenshotBase64
                })
            });
            step = await res.json();
        } catch (e) {
            console.error('Step decision request failed:', e);
            finalAnswer = 'Lost connection while figuring out the next step — stopping here.';
            break;
        }

        if (requestId !== activeRequestId) return; // aborted while awaiting

        console.log(`[multistep] step ${i + 1}:`, JSON.stringify(step));

        if (step.action_type === 'done') {
            finalAnswer = step.answer_text || 'Done.';
            break;
        }

        progressText.textContent = step.description || `Step ${i + 1}…`;

        try {
            if (step.action_type === 'click' && step.point) {
                // Gemini's native format is [y, x] normalized 0-1000 — same
                // convention already validated in storyboard mode.
                const yNorm = step.point[0] / 1000;
                const xNorm = step.point[1] / 1000;
                await invoke('execute_click', { xNorm, yNorm });
            } else if (step.action_type === 'type_text' && step.text) {
                // false: don't restore the pre-hotkey window's focus — this
                // step should type into whatever the sequence itself just
                // focused (e.g. Notepad after opening it), not yank focus
                // back to whatever was open before the hotkey was pressed.
                await invoke('execute_type_text', { text: step.text, restoreOriginalFocus: false });
            } else if (step.action_type === 'open_app' && step.app_name) {
                await invoke('execute_open_app', { appName: step.app_name });
            } else if (step.action_type === 'key_press' && step.key) {
                await invoke('execute_key_press', { key: step.key });
            }
        } catch (e) {
            console.error('Step execution failed:', e);
        }

        completedSteps.push(step.description || step.action_type);
        // Launching an app is much slower to actually render than a
        // click/type/key press — the fixed delay wasn't enough for it in
        // testing (a fresh screenshot could still show the desktop/Start
        // menu, mid-launch), which made the model think the open failed and
        // repeat it. Rust's execute_open_app also grew its own internal
        // settle time; this is on top of that, before the NEXT step's
        // screenshot is taken.
        const settleMs = step.action_type === 'open_app' ? 2200 : MULTI_STEP_STEP_DELAY_MS;
        await new Promise((r) => setTimeout(r, settleMs));
    }

    if (requestId !== activeRequestId) return; // aborted during the final delay

    progressPill.classList.add('hidden');

    if (!finalAnswer) {
        finalAnswer = `Stopped after ${MULTI_STEP_MAX_STEPS} steps without finishing — try a narrower request.`;
    }

    // No TTS narration for the multi-step summary yet — deliberate v1 scope
    // cut, not an oversight; text-only final answer, same dismiss-timing
    // math as a spoken-off normal answer.
    getOrCreateTooltip();
    document.getElementById('answer-text').textContent = finalAnswer;
    document.getElementById('answer-caret').classList.add('hidden');
    if (closeTimer) clearTimeout(closeTimer);
    const wordCount = finalAnswer.split(/\s+/).length;
    closeTimer = setTimeout(() => dismissOverlay(), Math.max(15000, (wordCount / 2.5) * 1000 + 4000));
}

btnSubmit.addEventListener('click', async () => {
    if (!currentRect) return;
    // Snapshot: currentRect is a shared global that Escape, a new selection,
    // or a primary-hotkey press elsewhere can null out while we're awaiting
    // the backend. Reading currentRect again after the await (as this used
    // to) could hand renderResponse a null rect and throw.
    const rect = currentRect;
    const requestId = ++activeRequestId;

    const rawQuery = queryInput.value.trim();
    const agentMatch = rawQuery.match(/^agent:\s*(.*)$/i);

    btnSubmit.disabled = true;
    btnSubmit.innerText = agentMatch ? "Queueing..." : "Processing...";
    btnCancel.disabled = true;

    img.style.display = 'none';
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');

    const appWindow = Window.getCurrent();
    await appWindow.hide();

    try {
        let response;
        let agentTaskDescription = null;
        if (agentMatch) {
            agentTaskDescription = agentMatch[1] || "Analyze this UI area for agent actions";
            response = await runAgentTask(agentTaskDescription);
        } else {
            response = await invoke('process_crop', {
                rect,
                query: rawQuery || null,
                requestId: String(requestId)
            });
        }

        if (requestId !== activeRequestId) return; // superseded while we were waiting

        // Show window again to render the result
        await appWindow.setIgnoreCursorEvents(true);
        await appWindow.show();
        await appWindow.setFocus();
        await enableEscapeDismiss();

        if (response.multi_step_plan) {
            await runMultiStepLoop(agentTaskDescription, response.multi_step_plan);
            return;
        }

        await confirmProposedActionIfAny(response.proposed_action);
        if (requestId !== activeRequestId) return; // could've been superseded during the confirm wait

        renderResponse(response, rect);
    } catch (error) {
        if (requestId !== activeRequestId) return;

        console.error("Error processing request:", error);
        // Window may be hidden (error before the show() above) or already
        // click-through (error from renderResponse, after it) — cover both
        // so the alert's OK button is always reachable.
        await appWindow.show();
        await appWindow.setFocus();
        await appWindow.setIgnoreCursorEvents(false);
        await disableEscapeDismiss();
        resetSelection();
        showError(`${error}`, async () => {
            await appWindow.hide();
            await appWindow.setIgnoreCursorEvents(false);
        });
    } finally {
        btnSubmit.disabled = false;
        btnSubmit.innerText = "Process";
        btnCancel.disabled = false;
    }
});

// Shared dismiss path for the answer tooltip: auto-timeout, Escape, and the
// global-shortcut Escape all funnel through here so the window always ends
// up hidden and non-ignoring (ready for the next hotkey press).
async function dismissOverlay() {
    if (closeTimer) {
        clearTimeout(closeTimer);
        closeTimer = null;
    }

    // Invalidate any request still in flight — without this, a response (or
    // stream chunk) that arrives after the user has already dismissed would
    // still pass the requestId check (no *newer* request has started) and
    // re-show the window with a stale answer.
    activeRequestId++;

    const tooltip = document.getElementById('answer-tooltip');
    if (tooltip) tooltip.remove();
    document.getElementById('storyboard-bar').classList.add('hidden');
    document.getElementById('storyboard-hint').classList.add('hidden');
    clearStoryboardShapes();
    document.getElementById('action-confirm-box').classList.add('hidden');
    document.getElementById('action-running-pill').classList.add('hidden');
    document.getElementById('multistep-confirm-box').classList.add('hidden');
    document.getElementById('multistep-progress-pill').classList.add('hidden');
    loadingIndicator.classList.add('hidden');
    if (errorTimer) { clearTimeout(errorTimer); errorTimer = null; }
    errorToast.classList.add('hidden');

    invoke('stop_speech').catch((e) => console.error("Failed to stop speech:", e));
    stopSpeakingIndicator();

    const appWindow = Window.getCurrent();
    await appWindow.hide();
    await appWindow.setIgnoreCursorEvents(false);
    await disableEscapeDismiss();
}

// Backend clamps pointer_target to [0, 1] before it's sent, but that's not a
// hard contract on what Gemini actually returns — defend here too, since an
// unclamped value multiplied into rect.width/height can throw the tooltip
// thousands of px off-screen (silent, no error, just nothing visible).
const clampUnit = (n) => Math.max(0, Math.min(1, Number(n) || 0));

function renderResponse(response, rect) {
    // Remove any leftover marker from a previous cycle
    const oldMarker = document.getElementById('pointer-marker');
    if (oldMarker) oldMarker.remove();

    // Calculate position
    let targetX = rect.x + (rect.width / 2);
    let targetY = rect.y + (rect.height / 2);

    if (response.pointer_target) {
        // x_norm and y_norm are relative to the crop!
        targetX = rect.x + (clampUnit(response.pointer_target.x_norm) * rect.width);
        targetY = rect.y + (clampUnit(response.pointer_target.y_norm) * rect.height);

        // Render marker
        const marker = document.createElement('div');
        marker.id = 'pointer-marker';
        marker.className = 'pointer-marker';
        marker.style.left = targetX + 'px';
        marker.style.top = targetY + 'px';
        container.appendChild(marker);
    }

    // Reuse the tooltip the streaming listener already built up live, if any
    // (falls back to creating one — e.g. a response with zero chunks).
    // No close button on the bubble itself: the window runs with
    // setIgnoreCursorEvents(true) while the answer is shown (deliberate
    // "fully click-through" behavior so the overlay never blocks the app
    // underneath), which means the OS drops all mouse events before they
    // reach the webview — a close button here couldn't be clicked. Esc is
    // the dismiss path. Position is fixed via CSS (top-anchored, centered)
    // rather than anchored to pointer_target — anchoring next to it risked
    // pushing the box past the viewport edge (clipped, invisible, no error)
    // whenever the model's coordinate landed near a corner.
    getOrCreateTooltip();
    document.getElementById('answer-text').textContent = response.answer_text; // resync: authoritative final text
    document.getElementById('answer-caret').classList.add('hidden');

    // Word-count estimate — the real dismiss timer only when speech is
    // disabled/fails; the instant we confirm speech IS playing (below), this
    // gets cancelled outright rather than left running. It used to only get
    // cancelled once narration finished, which meant it could still fire on
    // its own short schedule while a longer message was still being spoken —
    // killing both audio and the overlay mid-sentence. Now nothing but real
    // narration end (or its own generous fallback ceiling, see
    // startSpeakingIndicator) decides when a spoken answer dismisses.
    if (closeTimer) clearTimeout(closeTimer);
    const wordCount = response.answer_text ? response.answer_text.split(/\s+/).length : 0;
    const displayMs = Math.max(15000, (wordCount / 2.5) * 1000 + 4000);
    closeTimer = setTimeout(() => dismissOverlay(), displayMs);

    if (response.answer_text) {
        invoke('get_speech_enabled')
            .then(async (enabled) => {
                if (enabled) {
                    // Speech is about to play — the word-count estimate above
                    // no longer applies at all, narration owns dismiss timing
                    // from here. Once it actually finishes (real tts-ended
                    // event, or its own fallback ceiling), give the user a
                    // short moment to glance at the text, then dismiss.
                    if (closeTimer) clearTimeout(closeTimer);
                    const header = document.querySelector('#answer-tooltip .answer-header');
                    await startSpeakingIndicator(response.answer_text, () => {
                        if (closeTimer) clearTimeout(closeTimer);
                        closeTimer = setTimeout(() => dismissOverlay(), 5000);
                    }, header);
                    return invoke('speak_text', { text: response.answer_text, voiceId: null });
                }
            })
            .catch((e) => {
                console.error("TTS failed:", e);
                stopSpeakingIndicator();
            });
    }

    // Hide the selection box and toolbar so the user can see the result clearly
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');
}

// Close overlay on Escape
document.addEventListener('keydown', async (e) => {
    if (e.key === 'Escape') {
        await dismissOverlay();
        resetSelection();
    }
});
