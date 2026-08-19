const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const panel = document.getElementById('panel');
const statusEl = document.getElementById('status');
const updateLine = document.getElementById('update-line');

// One-shot startup confirmation, not a loop — fades in, shows "Starting…",
// swaps to a ready message once the window itself exists (this window only
// gets created after Tauri's setup() has already registered the hotkey and
// tray, so "ready" here is honest, not aspirational), then fades out and
// closes itself. No buttons, no interaction needed — except the optional
// update line below, which is clickable.
async function run() {
    requestAnimationFrame(() => panel.classList.add('visible'));

    // Kicked off in parallel with the fade/status cycle, not awaited before
    // it — a slow/offline network shouldn't delay the splash itself.
    const updateCheck = checkForUpdate();

    await sleep(900);
    statusEl.style.opacity = '0';

    await sleep(180);
    statusEl.textContent = 'Ready — press Ctrl+Alt+Space anytime';
    statusEl.style.opacity = '1';

    const update = await updateCheck;
    let holdMs = 1800;
    if (update) {
        updateLine.textContent = `Update available — v${update.version}`;
        updateLine.addEventListener('click', () => {
            invoke('open_release_page', { url: update.url }).catch((e) =>
                console.error('Failed to open release page:', e)
            );
        });
        updateLine.classList.add('visible');
        holdMs = 4000; // give the user an actual chance to see/click it
    }

    await sleep(holdMs);
    panel.classList.remove('visible');
    panel.classList.add('leaving');

    await sleep(520);
    try {
        await Window.getCurrent().close();
    } catch (e) {
        console.error('Failed to close splash window:', e);
    }
}

async function checkForUpdate() {
    try {
        return await invoke('check_for_update');
    } catch (e) {
        console.error('Update check failed:', e);
        return null;
    }
}

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

run();
