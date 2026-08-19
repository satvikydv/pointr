const { Window } = window.__TAURI__.window;

const panel = document.getElementById('panel');
const statusEl = document.getElementById('status');

// One-shot startup confirmation, not a loop — fades in, shows "Starting…",
// swaps to a ready message once the window itself exists (this window only
// gets created after Tauri's setup() has already registered the hotkey and
// tray, so "ready" here is honest, not aspirational), then fades out and
// closes itself. No buttons, no interaction needed.
async function run() {
    requestAnimationFrame(() => panel.classList.add('visible'));

    await sleep(900);
    statusEl.style.opacity = '0';

    await sleep(180);
    statusEl.textContent = 'Ready — press Ctrl+Alt+Space anytime';
    statusEl.style.opacity = '1';

    await sleep(1800);
    panel.classList.remove('visible');
    panel.classList.add('leaving');

    await sleep(520);
    try {
        await Window.getCurrent().close();
    } catch (e) {
        console.error('Failed to close splash window:', e);
    }
}

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

run();
