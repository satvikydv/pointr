const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const container = document.getElementById('overlay-container');
const img = document.getElementById('screenshot-img');
const selectionBox = document.getElementById('selection-box');
const toolbar = document.getElementById('toolbar');
const btnCancel = document.getElementById('btn-cancel');
const btnSubmit = document.getElementById('btn-submit');
const btnAgent = document.getElementById('btn-agent');

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

// Primary hotkey: fires immediately on hotkey press with no user input needed.
// Rust has already captured the screen, burned a marker in at the cursor
// position, and stashed it — we just kick off the backend call and render
// whatever comes back.
listen('show-overlay-direct', async () => {
    if (closeTimer) clearTimeout(closeTimer);
    mode = 'direct';

    img.style.display = 'none';
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');
    currentRect = null;

    const appWindow = Window.getCurrent();

    try {
        const response = await invoke('process_direct');

        await appWindow.setIgnoreCursorEvents(true);
        await appWindow.show();
        await appWindow.setFocus();

        const rect = { x: 0, y: 0, width: window.innerWidth, height: window.innerHeight };
        renderResponse(response, rect);
    } catch (error) {
        console.error("Error calling process_direct:", error);
        alert(`Error: ${error}`);
        await appWindow.hide();
    }
});

function resetSelection() {
    img.style.display = 'block';
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');
    currentRect = null;
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
});

btnCancel.addEventListener('click', async () => {
    const appWindow = Window.getCurrent();
    await appWindow.hide();
    resetSelection();
});

btnSubmit.addEventListener('click', async () => {
    if (currentRect) {
        console.log("Processing rect:", currentRect);
        
        // Disable buttons
        btnSubmit.disabled = true;
        btnSubmit.innerText = "Processing...";
        btnCancel.disabled = true;

        // Hide overlay UI and hide the entire window so user can keep working
        img.style.display = 'none';
        selectionBox.style.display = 'none';
        toolbar.classList.add('hidden');
        
        const appWindow = Window.getCurrent();
        await appWindow.hide();

        try {
            const response = await invoke('process_crop', { 
                rect: currentRect,
                query: null // For now, no text input in UI
            });
            
            // Show window again to render the result
            await appWindow.setIgnoreCursorEvents(true);
            await appWindow.show();
            // Ensure window is brought to front
            await appWindow.setFocus();
            
            renderResponse(response, currentRect);
        } catch (error) {
            console.error("Error calling process_crop:", error);
            alert(`Error: ${error}`);
            resetSelection();
        } finally {
            // Reset loading state for next time
            btnSubmit.disabled = false;
            btnSubmit.innerText = "Process";
            btnCancel.disabled = false;
        }
    }
});

btnAgent.addEventListener('click', async () => {
    if (currentRect) {
        console.log("Testing Agent on rect:", currentRect);
        
        btnSubmit.disabled = true;
        btnAgent.disabled = true;
        btnAgent.innerText = "Queueing...";
        btnCancel.disabled = true;

        img.style.display = 'none';
        selectionBox.style.display = 'none';
        toolbar.classList.add('hidden');
        
        const appWindow = Window.getCurrent();
        await appWindow.hide();

        try {
            // 1. Queue task
            const postRes = await fetch("http://localhost:8000/api/agent/task", {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({
                    task_description: "Analyze this UI area for agent actions",
                    session_id: "test-session-123"
                })
            });
            const { task_id } = await postRes.json();
            
            // 2. Poll for completion
            let isDone = false;
            let finalResult = null;
            
            while (!isDone) {
                await new Promise(r => setTimeout(r, 1000));
                
                const getRes = await fetch(`http://localhost:8000/api/agent/task/${task_id}`);
                const statusData = await getRes.json();
                
                if (statusData.status === "SUCCESS") {
                    isDone = true;
                    finalResult = statusData.result;
                } else if (statusData.status === "FAILURE") {
                    isDone = true;
                    throw new Error("Task failed: " + statusData.result);
                }
            }
            
            // Show window again
            await appWindow.setIgnoreCursorEvents(true);
            await appWindow.show();
            await appWindow.setFocus();
            
            // Re-use renderResponse
            renderResponse({
                answer_text: finalResult.result,
                pointer_target: finalResult.pointer_target
            }, currentRect);
            
        } catch (error) {
            console.error("Agent error:", error);
            alert(`Agent Error: ${error}`);
            resetSelection();
        } finally {
            btnSubmit.disabled = false;
            btnAgent.disabled = false;
            btnAgent.innerText = "Test Agent";
            btnCancel.disabled = false;
        }
    }
});

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
    // since the user can't click it (ignoreCursorEvents is true)
    if (closeTimer) clearTimeout(closeTimer);
    closeTimer = setTimeout(async () => {
        if (tooltip) tooltip.remove();
        const marker = document.getElementById('pointer-marker');
        if (marker) marker.remove();
        
        const appWindow = Window.getCurrent();
        await appWindow.hide();
        await appWindow.setIgnoreCursorEvents(false);
    }, 15000);
}

// Close overlay on Escape
document.addEventListener('keydown', async (e) => {
    if (e.key === 'Escape') {
        const appWindow = Window.getCurrent();
        await appWindow.hide();
        await appWindow.setIgnoreCursorEvents(false);
        resetSelection();
        
        const oldMarker = document.getElementById('pointer-marker');
        if (oldMarker) oldMarker.remove();
        const oldTooltip = document.getElementById('answer-tooltip');
        if (oldTooltip) oldTooltip.remove();
    }
});
