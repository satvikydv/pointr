const { invoke } = window.__TAURI__.core;

const voiceSelect = document.getElementById('voice-select');
const btnTest = document.getElementById('btn-test');
const btnSave = document.getElementById('btn-save');
const status = document.getElementById('status');

function showStatus(text) {
    status.textContent = text;
    setTimeout(() => {
        if (status.textContent === text) status.textContent = '';
    }, 2500);
}

async function init() {
    let voices = [];
    try {
        voices = await invoke('list_voices');
    } catch (e) {
        showStatus(`Failed to load voices: ${e}`);
        return;
    }

    voiceSelect.innerHTML = '';
    for (const voice of voices) {
        const option = document.createElement('option');
        option.value = voice.id;
        option.textContent = `${voice.display_name} (${voice.language})`;
        voiceSelect.appendChild(option);
    }

    try {
        const selected = await invoke('get_selected_voice');
        if (selected) voiceSelect.value = selected;
    } catch (e) {
        console.error('Failed to load current voice selection:', e);
    }
}

btnTest.addEventListener('click', async () => {
    const voiceId = voiceSelect.value;
    if (!voiceId) return;
    btnTest.disabled = true;
    try {
        await invoke('speak_text', {
            text: 'This is a preview of the selected voice.',
            voiceId,
        });
    } catch (e) {
        showStatus(`Preview failed: ${e}`);
    } finally {
        btnTest.disabled = false;
    }
});

btnSave.addEventListener('click', async () => {
    const voiceId = voiceSelect.value;
    if (!voiceId) return;
    try {
        await invoke('set_selected_voice', { voiceId });
        showStatus('Saved.');
    } catch (e) {
        showStatus(`Failed to save: ${e}`);
    }
});

init();
