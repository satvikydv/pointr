const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const toggleTrack = document.getElementById('toggle-track');
const toggleKnob = document.getElementById('toggle-knob');
const osToggleTrack = document.getElementById('os-toggle-track');
const osToggleKnob = document.getElementById('os-toggle-knob');
const voiceSection = document.getElementById('voice-section');
const voiceTrigger = document.getElementById('voice-trigger');
const voiceTriggerLabel = document.getElementById('voice-trigger-label');
const voiceChevron = voiceTrigger.querySelector('.chevron');
const voiceDropdown = document.getElementById('voice-dropdown');
const voicePickerWrap = document.getElementById('voice-picker-wrap');
const btnTest = document.getElementById('btn-test');
const testDots = document.getElementById('test-dots');
const testLabel = document.getElementById('test-label');
const btnSave = document.getElementById('btn-save');
const btnClose = document.getElementById('btn-close');
const statusEl = document.getElementById('status');

const state = {
    audioOn: true,
    osActionsOn: true,
    voices: [], // {id, display_name, language}
    selectedId: null,
    savedId: null,
    dropdownOpen: false,
    isPlaying: false,
    statusText: '',
    statusIsUnsaved: false,
};

let statusTimer = null;
let playTimer = null;

function voiceLabel(voice) {
    return voice ? `${voice.display_name} (${voice.language})` : 'No voices found';
}

function render() {
    // Toggle switch
    toggleTrack.style.background = state.audioOn ? '#5b8cff' : 'rgba(255,255,255,0.12)';
    toggleTrack.style.borderColor = state.audioOn ? '#5b8cff' : 'rgba(255,255,255,0.16)';
    toggleTrack.style.border = `1px solid ${state.audioOn ? '#5b8cff' : 'rgba(255,255,255,0.16)'}`;
    toggleKnob.style.left = (state.audioOn ? 18 : 1) + 'px';

    osToggleTrack.style.background = state.osActionsOn ? '#5b8cff' : 'rgba(255,255,255,0.12)';
    osToggleTrack.style.border = `1px solid ${state.osActionsOn ? '#5b8cff' : 'rgba(255,255,255,0.16)'}`;
    osToggleKnob.style.left = (state.osActionsOn ? 18 : 1) + 'px';

    // Voice section disabled look when audio is off
    voiceSection.style.opacity = state.audioOn ? '1' : '0.45';
    voiceSection.style.pointerEvents = state.audioOn ? 'auto' : 'none';

    // Trigger label + chevron
    const selected = state.voices.find((v) => v.id === state.selectedId);
    voiceTriggerLabel.textContent = state.voices.length ? voiceLabel(selected) : 'No voices found';
    voiceChevron.style.transform = state.dropdownOpen ? 'rotate(225deg) translate(-2px,-2px)' : 'rotate(45deg)';

    // Dropdown list
    voiceDropdown.classList.toggle('hidden', !state.dropdownOpen);
    voiceDropdown.innerHTML = '';
    for (const voice of state.voices) {
        const row = document.createElement('div');
        row.className = 'voice-row';
        row.style.background = voice.id === state.selectedId ? 'rgba(91,140,255,0.12)' : 'transparent';
        const name = document.createElement('div');
        name.className = 'name';
        name.textContent = voiceLabel(voice);
        row.appendChild(name);
        if (voice.id === state.selectedId) {
            const check = document.createElement('div');
            check.className = 'check';
            check.textContent = '✓';
            row.appendChild(check);
        }
        row.addEventListener('click', () => selectVoice(voice.id));
        voiceDropdown.appendChild(row);
    }

    // Test button
    testDots.classList.toggle('visible', state.isPlaying);
    testLabel.textContent = state.isPlaying ? 'Playing…' : 'Test';
    btnTest.style.opacity = state.audioOn ? '1' : '0.5';
    btnTest.style.cursor = state.audioOn ? 'pointer' : 'default';

    // Status
    statusEl.textContent = state.statusText;
    statusEl.style.color = state.statusIsUnsaved ? 'rgba(242,184,75,0.85)' : 'rgba(140,220,170,0.9)';
}

function selectVoice(id) {
    state.selectedId = id;
    state.dropdownOpen = false;
    const dirty = id !== state.savedId;
    state.statusText = dirty ? 'Unsaved change' : '';
    state.statusIsUnsaved = dirty;
    render();
}

voiceTrigger.addEventListener('click', () => {
    if (!state.audioOn || !state.voices.length) return;
    state.dropdownOpen = !state.dropdownOpen;
    render();
});

document.addEventListener('click', (e) => {
    if (state.dropdownOpen && !voicePickerWrap.contains(e.target)) {
        state.dropdownOpen = false;
        render();
    }
});

toggleTrack.addEventListener('click', async () => {
    state.audioOn = !state.audioOn;
    state.dropdownOpen = false;
    render();
    try {
        await invoke('set_speech_enabled', { enabled: state.audioOn });
    } catch (e) {
        console.error('Failed to save speech-enabled setting:', e);
    }
});

osToggleTrack.addEventListener('click', async () => {
    state.osActionsOn = !state.osActionsOn;
    render();
    try {
        await invoke('set_os_actions_enabled', { enabled: state.osActionsOn });
    } catch (e) {
        console.error('Failed to save os-actions-enabled setting:', e);
    }
});

btnTest.addEventListener('click', async () => {
    if (!state.audioOn || state.isPlaying || !state.selectedId) return;
    state.isPlaying = true;
    render();
    clearTimeout(playTimer);
    playTimer = setTimeout(() => {
        state.isPlaying = false;
        render();
    }, 1800);
    try {
        await invoke('speak_text', {
            text: 'This is a preview of the selected voice.',
            voiceId: state.selectedId,
        });
    } catch (e) {
        console.error('TTS preview failed:', e);
    }
});

btnSave.addEventListener('click', async () => {
    if (!state.selectedId) return;
    try {
        await invoke('set_selected_voice', { voiceId: state.selectedId });
        state.savedId = state.selectedId;
        state.statusText = 'Saved.';
        state.statusIsUnsaved = false;
        render();
        clearTimeout(statusTimer);
        statusTimer = setTimeout(() => {
            state.statusText = '';
            render();
        }, 2200);
    } catch (e) {
        state.statusText = `Failed to save: ${e}`;
        state.statusIsUnsaved = true;
        render();
    }
});

btnClose.addEventListener('click', async () => {
    await Window.getCurrent().hide();
});

async function init() {
    try {
        state.voices = await invoke('list_voices');
    } catch (e) {
        console.error('Failed to load voices:', e);
    }

    try {
        state.audioOn = await invoke('get_speech_enabled');
    } catch (e) {
        console.error('Failed to load speech-enabled setting:', e);
    }

    try {
        state.osActionsOn = await invoke('get_os_actions_enabled');
    } catch (e) {
        console.error('Failed to load os-actions-enabled setting:', e);
    }

    try {
        const saved = await invoke('get_selected_voice');
        state.savedId = saved;
        state.selectedId = saved && state.voices.some((v) => v.id === saved)
            ? saved
            : (state.voices[0] ? state.voices[0].id : null);
    } catch (e) {
        console.error('Failed to load current voice selection:', e);
        state.selectedId = state.voices[0] ? state.voices[0].id : null;
    }

    render();
}

init();
