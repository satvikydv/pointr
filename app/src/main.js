const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const container = document.getElementById('overlay-container');
const img = document.getElementById('screenshot-img');
const selectionBox = document.getElementById('selection-box');
const toolbar = document.getElementById('toolbar');
const btnCancel = document.getElementById('btn-cancel');
const btnSubmit = document.getElementById('btn-submit');

let isDrawing = false;
let startX = 0;
let startY = 0;
let currentRect = null;

// Listen for the overlay trigger
listen('show-overlay', (event) => {
    img.src = event.payload;
    resetSelection();
});

function resetSelection() {
    selectionBox.style.display = 'none';
    toolbar.classList.add('hidden');
    currentRect = null;
}

container.addEventListener('mousedown', (e) => {
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
        
        // Show loading state
        btnSubmit.disabled = true;
        btnSubmit.innerText = "Processing...";
        btnCancel.disabled = true;

        try {
            const response = await invoke('process_crop', { 
                rect: currentRect,
                query: null // For now, no text input in UI
            });
            
            alert(`Response: ${response.answer_text}`);
        } catch (error) {
            console.error("Error calling process_crop:", error);
            alert(`Error: ${error}`);
        } finally {
            // Reset loading state
            btnSubmit.disabled = false;
            btnSubmit.innerText = "Process";
            btnCancel.disabled = false;
            
            // Hide overlay
            const appWindow = Window.getCurrent();
            await appWindow.hide();
            resetSelection();
        }
    }
});

// Close overlay on Escape
document.addEventListener('keydown', async (e) => {
    if (e.key === 'Escape') {
        const appWindow = Window.getCurrent();
        await appWindow.hide();
        resetSelection();
    }
});
