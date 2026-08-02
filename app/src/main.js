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

let isDrawing = false;
let startX = 0;
let startY = 0;
let currentRect = null;
let closeTimer = null;
// 'region' = manual drag-to-crop flow (secondary hotkey, Ctrl+Alt+Shift+Space)
// 'direct' = single-press capture + auto-send flow (primary hotkey, Ctrl+Alt+Space)
let mode = 'region';

// Secondary hotkey: manual region-select flow. Shows the full screenshot and
// waits for the user to drag a crop box before doing anything.
listen('show-overlay', async (event) => {
    if (closeTimer) clearTimeout(closeTimer);
    mode = 'region';

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

// Guards against overlapping presses: each press fires this handler again
// independently, so if a prior press's backend call is still pending when a
// new one starts, both run concurrently and whichever resolves last would
// otherwise win the render — showing a stale answer from an earlier press
// (PRD §8: "second press should refresh capture... not stack a second
// overlay"). Only the response matching the most recent press is rendered.
let activeDirectRequestId = 0;

// Primary hotkey: fires immediately on hotkey press with no user input needed.
// Rust has already captured the screen, burned a marker in at the cursor
// position, and stashed it — we just kick off the backend call and render
// whatever comes back.
listen('show-overlay-direct', async () => {
    if (closeTimer) clearTimeout(closeTimer);
    mode = 'direct';

    const requestId = ++activeDirectRequestId;

    img.style.display = 'none';
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');
    currentRect = null;

    const appWindow = Window.getCurrent();

    // Show the window right away with a "Thinking..." badge so the user gets
    // immediate feedback that the hotkey registered and a request is in
    // flight, instead of staring at nothing until the backend responds.
    // Stays click-through (ignoreCursorEvents true) — it's status-only, not
    // interactive, so it must not block clicks to whatever's underneath.
    loadingIndicator.classList.remove('hidden');
    await appWindow.setIgnoreCursorEvents(true);
    await appWindow.show();
    await enableEscapeDismiss();

    try {
        const response = await invoke('process_direct');

        if (requestId !== activeDirectRequestId) return; // superseded by a later press

        loadingIndicator.classList.add('hidden');
        await appWindow.setFocus();

        const rect = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
        renderResponse(response, rect);
    } catch (error) {
        if (requestId !== activeDirectRequestId) return;

        console.error("Error calling process_direct:", error);
        loadingIndicator.classList.add('hidden');
        // Window may still be click-through from the setIgnoreCursorEvents(true)
        // above — flip it off *before* alert() so the OK button is reachable.
        await appWindow.setFocus();
        await appWindow.setIgnoreCursorEvents(false);
        alert(`Error: ${error}`);
        await appWindow.hide();
        await appWindow.setIgnoreCursorEvents(false);
        await disableEscapeDismiss();
    }
});

function resetSelection() {
    img.style.display = 'block';
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');
    currentRect = null;
    queryInput.value = '';
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
                query: rawQuery || null
            });
        }

        // Show window again to render the result
        await appWindow.setIgnoreCursorEvents(true);
        await appWindow.show();
        await appWindow.setFocus();
        await enableEscapeDismiss();

        renderResponse(response, rect);
    } catch (error) {
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
// tooltip's own close button all funnel through here so the window always
// ends up hidden and non-ignoring (ready for the next hotkey press).
async function dismissOverlay() {
    if (closeTimer) {
        clearTimeout(closeTimer);
        closeTimer = null;
    }

    // Invalidate any direct-mode request still in flight — without this, a
    // response that arrives after the user has already dismissed would still
    // pass the requestId check (no *newer* press has started) and re-show
    // the window with a stale answer.
    activeDirectRequestId++;

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

function renderResponse(response, rect) {
    // Remove old marker/tooltip if any
    const oldMarker = document.getElementById('pointer-marker');
    if (oldMarker) oldMarker.remove();
    const oldTooltip = document.getElementById('answer-tooltip');
    if (oldTooltip) oldTooltip.remove();

    // Calculate position
    let targetX = rect.x + (rect.width / 2);
    let targetY = rect.y + (rect.height / 2);

    if (response.pointer_target) {
        // x_norm and y_norm are relative to the crop!
        targetX = rect.x + (response.pointer_target.x_norm * rect.width);
        targetY = rect.y + (response.pointer_target.y_norm * rect.height);

        // Render marker
        const marker = document.createElement('div');
        marker.id = 'pointer-marker';
        marker.className = 'pointer-marker';
        marker.style.left = targetX + 'px';
        marker.style.top = targetY + 'px';
        container.appendChild(marker);
    }

    // Render tooltip
    // No close button: the window runs with setIgnoreCursorEvents(true) while
    // the answer is shown (deliberate "fully click-through" behavior so the
    // overlay never blocks the app underneath), which means the OS drops all
    // mouse events before they reach the webview — a button here would never
    // actually receive a click. Esc is the dismiss path instead.
    const tooltip = document.createElement('div');
    tooltip.id = 'answer-tooltip';
    tooltip.className = 'answer-tooltip';
    tooltip.innerText = response.answer_text;

    // Position tooltip near the target
    tooltip.style.left = (targetX + 20) + 'px';
    tooltip.style.top = (targetY + 20) + 'px';

    container.appendChild(tooltip);

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
