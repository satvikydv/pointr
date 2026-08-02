const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const container = document.getElementById('overlay-container');
const img = document.getElementById('screenshot-img');
const selectionBox = document.getElementById('selection-box');
const toolbar = document.getElementById('toolbar');
const btnCancel = document.getElementById('btn-cancel');
const btnSubmit = document.getElementById('btn-submit');
const queryInput = document.getElementById('query-input');
const loadingIndicator = document.getElementById('loading-indicator');
const directQueryBox = document.getElementById('direct-query-box');
const directQueryInput = document.getElementById('direct-query-input');

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
    console.log('[analyze-stream-chunk]', { request_id, activeRequestId, matched: String(activeRequestId) === request_id, textLen: text?.length });
    if (String(activeRequestId) !== request_id) return;

    loadingIndicator.classList.add('hidden');

    let tooltip = document.getElementById('answer-tooltip');
    if (!tooltip) {
        tooltip = document.createElement('div');
        tooltip.id = 'answer-tooltip';
        tooltip.className = 'answer-tooltip';
        // No pointer_target yet (only known once the stream finishes), so
        // anchor the growing answer centered; renderResponse() repositions
        // it next to the marker once the final response arrives.
        tooltip.style.left = '50%';
        tooltip.style.top = '40%';
        tooltip.style.transform = 'translate(-50%, -50%)';
        container.appendChild(tooltip);
    }
    tooltip.textContent += text;
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

    loadingIndicator.textContent = 'Thinking…';
    loadingIndicator.classList.remove('hidden');
    await appWindow.setIgnoreCursorEvents(true);
    await enableEscapeDismiss();

    console.log('[runDirectAnalysis] invoking process_direct', { requestId, queryText });

    try {
        const response = await invoke('process_direct', {
            query: queryText || null,
            requestId: String(requestId)
        });

        console.log('[runDirectAnalysis] process_direct resolved', { requestId, activeRequestId, response });

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

        const rect = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
        renderResponse(response, rect);

        console.log('[runDirectAnalysis] post-render window state', {
            requestId,
            visible: await appWindow.isVisible(),
            minimized: await appWindow.isMinimized(),
            position: await appWindow.outerPosition(),
            size: await appWindow.outerSize(),
            tooltipRect: document.getElementById('answer-tooltip')?.getBoundingClientRect(),
        });
    } catch (error) {
        console.log('[runDirectAnalysis] process_direct rejected', { requestId, activeRequestId, error });
        if (requestId !== activeRequestId) return;

        console.error("Error calling process_direct:", error);
        loadingIndicator.classList.add('hidden');
        // Window may still be click-through from the setIgnoreCursorEvents(true)
        // above — flip it off *before* alert() so the OK button is reachable.
        await appWindow.show();
        await appWindow.setFocus();
        await appWindow.setIgnoreCursorEvents(false);
        alert(`Error: ${error}`);
        await appWindow.hide();
        await appWindow.setIgnoreCursorEvents(false);
        await disableEscapeDismiss();
    }
}

function resetSelection() {
    img.style.display = 'block';
    selectionBox.style.display = 'none';
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

// Poll cap: 20 attempts * 1s = 20s, matching PRD §8's client-side timeout for
// a backend that never reaches a terminal state (worker down, stuck task).
const AGENT_POLL_MAX_ATTEMPTS = 20;
const AGENT_POLL_INTERVAL_MS = 1000;

async function runAgentTask(taskDescription) {
    const sessionId = crypto.randomUUID();

    const postRes = await fetch("http://localhost:8000/api/agent/task", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
            task_description: taskDescription,
            session_id: sessionId
        })
    });
    const { task_id } = await postRes.json();

    for (let attempt = 0; attempt < AGENT_POLL_MAX_ATTEMPTS; attempt++) {
        await new Promise(r => setTimeout(r, AGENT_POLL_INTERVAL_MS));

        const getRes = await fetch(`http://localhost:8000/api/agent/task/${task_id}`);
        const statusData = await getRes.json();

        if (statusData.status === "SUCCESS") {
            return {
                answer_text: statusData.result.result,
                pointer_target: statusData.result.pointer_target
            };
        } else if (statusData.status === "FAILURE") {
            throw new Error("Task failed: " + statusData.result);
        }
    }

    throw new Error(`Agent task timed out after ${AGENT_POLL_MAX_ATTEMPTS * AGENT_POLL_INTERVAL_MS / 1000}s`);
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
        if (agentMatch) {
            const taskDescription = agentMatch[1] || "Analyze this UI area for agent actions";
            response = await runAgentTask(taskDescription);
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
        alert(`Error: ${error}`);
        resetSelection();
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
    const marker = document.getElementById('pointer-marker');
    if (marker) marker.remove();
    loadingIndicator.classList.add('hidden');

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
    // No close button: the window runs with setIgnoreCursorEvents(true) while
    // the answer is shown (deliberate "fully click-through" behavior so the
    // overlay never blocks the app underneath), which means the OS drops all
    // mouse events before they reach the webview. Esc is the dismiss path.
    let tooltip = document.getElementById('answer-tooltip');
    if (!tooltip) {
        tooltip = document.createElement('div');
        tooltip.id = 'answer-tooltip';
        tooltip.className = 'answer-tooltip';
        container.appendChild(tooltip);
    }
    tooltip.textContent = response.answer_text; // resync: authoritative final text
    // Always centered — anchoring next to pointer_target risked pushing the
    // box past the viewport edge (clipped by overflow:hidden, invisible with
    // no error) whenever the model's coordinate landed near a corner. The
    // marker dot still goes to the actual pointer_target location; the text
    // box itself doesn't need to move with it to stay readable.
    tooltip.style.left = '50%';
    tooltip.style.top = '40%';
    tooltip.style.transform = 'translate(-50%, -50%)';

    // Hide the selection box and toolbar so the user can see the result clearly
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');

    // Automatically close and hide window after 15 seconds
    if (closeTimer) clearTimeout(closeTimer);
    closeTimer = setTimeout(() => dismissOverlay(), 15000);
}

// Close overlay on Escape
document.addEventListener('keydown', async (e) => {
    if (e.key === 'Escape') {
        await dismissOverlay();
        resetSelection();
    }
});
