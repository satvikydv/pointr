const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const toggleTrack = document.getElementById('toggle-track');
const toggleKnob = document.getElementById('toggle-knob');
const permissionButtons = document.querySelectorAll('#permission-picker button');
const permissionHint = document.getElementById('permission-hint');
const PERMISSION_HINTS = {
    off: 'Pointr only answers. It never types, clicks, or opens anything for you.',
    ask: 'Pointr shows what it is about to do and waits for Enter before acting. Esc cancels.',
    allow: 'Pointr acts straight away, with no prompt. Each step shows as it runs, and Esc stops it at any point.',
};
const telemetryToggleTrack = document.getElementById('telemetry-toggle-track');
const telemetryToggleKnob = document.getElementById('telemetry-toggle-knob');
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
const githubConnectedRow = document.getElementById('github-connected-row');
const githubTokenRow = document.getElementById('github-token-row');
const githubTokenInput = document.getElementById('github-token-input');
const btnGithubConnect = document.getElementById('btn-github-connect');
const btnGithubDisconnect = document.getElementById('btn-github-disconnect');

const state = {
    audioOn: true,
    // 'off' | 'ask' | 'allow' — replaced the old on/off OS actions toggle.
    actionPermission: 'ask',
    // Opt-in: mirrors the Rust default, and stays false if the real value
    // can't be read (fail closed, same as the capture path itself).
    telemetryOn: false,
    voices: [], // {id, display_name, language}
    selectedId: null,
    savedId: null,
    dropdownOpen: false,
    isPlaying: false,
    statusText: '',
    statusIsUnsaved: false,
    githubConnected: false,
    githubBusy: false,
};

let statusTimer = null;
let playTimer = null;

// BYOK: Gemini/Tavily key sections share identical markup/behavior (unlike
// GitHub's, which predates this and uses its own dedicated wiring below) —
// one generic setup function instead of duplicating connect/disconnect/
// status logic per key. Each section gets its own state.<prefix>Connected/
// state.<prefix>Busy fields, rendered generically in render() below.
function wireKeySection(prefix, { saveCmd, statusCmd, clearCmd, onChange }) {
    const connectedRow = document.getElementById(`${prefix}-connected-row`);
    const keyRow = document.getElementById(`${prefix}-key-row`);
    const input = document.getElementById(`${prefix}-key-input`);
    const btnConnect = document.getElementById(`btn-${prefix}-connect`);
    const btnDisconnect = document.getElementById(`btn-${prefix}-disconnect`);
    const connectedKey = `${prefix}Connected`;
    const busyKey = `${prefix}Busy`;
    state[connectedKey] = false;
    state[busyKey] = false;

    btnConnect.addEventListener('click', async () => {
        const key = input.value.trim();
        if (!key || state[busyKey]) return;
        state[busyKey] = true;
        render();
        try {
            await invoke(saveCmd, { key });
            input.value = '';
            state[connectedKey] = true;
            state.statusText = 'Saved.';
            state.statusIsUnsaved = false;
        } catch (e) {
            console.error(`Failed to save ${prefix} key:`, e);
            state.statusText = `Failed to connect: ${e}`;
            state.statusIsUnsaved = true;
        }
        state[busyKey] = false;
        render();
        if (onChange) onChange();
        clearTimeout(statusTimer);
        statusTimer = setTimeout(() => {
            state.statusText = '';
            render();
        }, 2200);
    });

    btnDisconnect.addEventListener('click', async () => {
        try {
            await invoke(clearCmd);
        } catch (e) {
            console.error(`Failed to clear ${prefix} key:`, e);
        }
        state[connectedKey] = false;
        render();
        if (onChange) onChange();
    });

    return {
        prefix, connectedRow, keyRow, btnConnect,
        async loadStatus() {
            try {
                state[connectedKey] = await invoke(statusCmd);
            } catch (e) {
                console.error(`Failed to load ${prefix} key status:`, e);
            }
        },
    };
}

const keySections = [
    wireKeySection('gemini', {
        saveCmd: 'save_gemini_key', statusCmd: 'get_gemini_key_status', clearCmd: 'clear_gemini_key',
        onChange: () => loadModelList('gemini'),
    }),
    wireKeySection('openai', {
        saveCmd: 'save_openai_key', statusCmd: 'get_openai_key_status', clearCmd: 'clear_openai_key',
        onChange: () => loadModelList('openai'),
    }),
    wireKeySection('tavily', { saveCmd: 'save_tavily_key', statusCmd: 'get_tavily_key_status', clearCmd: 'clear_tavily_key' }),
];

// ---------------------------------------------------------------------
// AI model: provider + model per provider. Rendered by renderModel(), not
// render(), since rebuilding the <select> on every render would close it
// mid-pick.
// ---------------------------------------------------------------------
const providerButtons = document.querySelectorAll('#provider-picker button');
const modelSelect = document.getElementById('model-select');
const customModelRow = document.getElementById('custom-model-row');
const customModelInput = document.getElementById('custom-model-input');
const btnCustomModel = document.getElementById('btn-custom-model');
const modelHint = document.getElementById('model-hint');
const CUSTOM_OPTION = '__custom__';
const PROVIDER_NAMES = { gemini: 'Gemini', openai: 'OpenAI' };

const modelState = {
    provider: 'gemini',
    chosen: { gemini: '', openai: '' },
    lists: { gemini: null, openai: null }, // {models, live} once loaded
    customOpen: false,
};

function flashSaved() {
    state.statusText = 'Saved.';
    state.statusIsUnsaved = false;
    render();
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
        state.statusText = '';
        render();
    }, 2200);
}

function renderModel() {
    const provider = modelState.provider;
    for (const b of providerButtons) {
        b.classList.toggle('active', b.dataset.provider === provider);
    }

    const chosen = modelState.chosen[provider];
    const list = modelState.lists[provider];
    const models = list ? [...list.models] : [];
    // A custom pick (or one the live list doesn't include) still shows as
    // the selected entry rather than silently displaying something else.
    if (chosen && !models.includes(chosen)) models.unshift(chosen);

    modelSelect.innerHTML = '';
    for (const id of models) {
        const opt = document.createElement('option');
        opt.value = id;
        opt.textContent = id;
        modelSelect.appendChild(opt);
    }
    const custom = document.createElement('option');
    custom.value = CUSTOM_OPTION;
    custom.textContent = 'Custom model ID…';
    modelSelect.appendChild(custom);
    modelSelect.value = modelState.customOpen ? CUSTOM_OPTION : chosen;

    customModelRow.classList.toggle('hidden', !modelState.customOpen);
    customModelInput.placeholder = provider === 'openai'
        ? 'Any model ID, e.g. gpt-6-sol'
        : 'Any model ID, e.g. gemini-3.5-flash';

    const name = PROVIDER_NAMES[provider];
    if (provider === 'openai' && !state.openaiConnected) {
        modelHint.textContent = 'Connect an OpenAI key below to use OpenAI models. Until then, requests will fail with a missing key message.';
    } else if (!list) {
        modelHint.textContent = 'Loading models…';
    } else if (list.live) {
        modelHint.textContent = `Every ${name} model your key can use with screenshots.`;
    } else {
        modelHint.textContent = `Suggested ${name} models. Connect your own ${name} key below to list every model it can use.`;
    }
}

async function loadModelList(provider) {
    try {
        modelState.lists[provider] = await invoke('list_models', { provider });
    } catch (e) {
        console.error(`Failed to list ${provider} models:`, e);
        modelState.lists[provider] = { models: [], live: false };
    }
    if (provider === modelState.provider) renderModel();
}

async function saveModel(model) {
    const provider = modelState.provider;
    try {
        await invoke('set_llm_model', { provider, model });
        modelState.chosen[provider] = model;
        modelState.customOpen = false;
        flashSaved();
    } catch (e) {
        console.error('Failed to save model:', e);
        state.statusText = `Failed to save: ${e}`;
        state.statusIsUnsaved = true;
        render();
    }
    renderModel();
}

for (const b of providerButtons) {
    b.addEventListener('click', async () => {
        const provider = b.dataset.provider;
        if (provider === modelState.provider) return;
        try {
            await invoke('set_llm_provider', { provider });
            modelState.provider = provider;
            modelState.customOpen = false;
            flashSaved();
        } catch (e) {
            console.error('Failed to save provider:', e);
        }
        renderModel();
    });
}

modelSelect.addEventListener('change', () => {
    if (modelSelect.value === CUSTOM_OPTION) {
        modelState.customOpen = true;
        customModelInput.value = '';
        renderModel();
        customModelInput.focus();
        return;
    }
    saveModel(modelSelect.value);
});

btnCustomModel.addEventListener('click', () => {
    const model = customModelInput.value.trim();
    if (model) saveModel(model);
});
customModelInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') btnCustomModel.click();
    if (e.key === 'Escape') {
        modelState.customOpen = false;
        renderModel();
    }
});

function voiceLabel(voice) {
    return voice ? `${voice.display_name} (${voice.language})` : 'No voices found';
}

function render() {
    // Toggle switch
    toggleTrack.style.background = state.audioOn ? '#5b8cff' : 'rgba(255,255,255,0.12)';
    toggleTrack.style.borderColor = state.audioOn ? '#5b8cff' : 'rgba(255,255,255,0.16)';
    toggleTrack.style.border = `1px solid ${state.audioOn ? '#5b8cff' : 'rgba(255,255,255,0.16)'}`;
    toggleKnob.style.left = (state.audioOn ? 18 : 1) + 'px';

    for (const b of permissionButtons) {
        b.classList.toggle('active', b.dataset.permission === state.actionPermission);
    }
    permissionHint.textContent = PERMISSION_HINTS[state.actionPermission] || '';
    telemetryToggleTrack.style.background = state.telemetryOn ? '#5b8cff' : 'rgba(255,255,255,0.12)';
    telemetryToggleTrack.style.border = `1px solid ${state.telemetryOn ? '#5b8cff' : 'rgba(255,255,255,0.16)'}`;
    telemetryToggleKnob.style.left = (state.telemetryOn ? 18 : 1) + 'px';

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

    // GitHub connection
    githubConnectedRow.classList.toggle('hidden', !state.githubConnected);
    githubTokenRow.classList.toggle('hidden', state.githubConnected);
    btnGithubConnect.disabled = state.githubBusy;
    btnGithubConnect.textContent = state.githubBusy ? 'Connecting…' : 'Connect';

    // BYOK key sections (Gemini, Tavily)
    for (const s of keySections) {
        const connected = state[`${s.prefix}Connected`];
        s.connectedRow.classList.toggle('hidden', !connected);
        s.keyRow.classList.toggle('hidden', connected);
        s.btnConnect.disabled = state[`${s.prefix}Busy`];
        s.btnConnect.textContent = state[`${s.prefix}Busy`] ? 'Connecting…' : 'Connect';
    }
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

for (const b of permissionButtons) {
    b.addEventListener('click', async () => {
        const permission = b.dataset.permission;
        if (permission === state.actionPermission) return;
        try {
            await invoke('set_action_permission', { permission });
            state.actionPermission = permission;
            flashSaved();
        } catch (e) {
            console.error('Failed to save action permission:', e);
        }
        render();
    });
}

telemetryToggleTrack.addEventListener('click', async () => {
    state.telemetryOn = !state.telemetryOn;
    render();
    try {
        await invoke('set_telemetry_enabled', { enabled: state.telemetryOn });
        // Flipping this in Settings also counts as answering the one-time
        // prompt, so it doesn't ask again afterwards.
        await invoke('set_telemetry_prompt_shown', { shown: true });
    } catch (e) {
        console.error('Failed to save telemetry-enabled setting:', e);
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

btnGithubConnect.addEventListener('click', async () => {
    const token = githubTokenInput.value.trim();
    if (!token || state.githubBusy) return;
    state.githubBusy = true;
    render();
    try {
        await invoke('save_github_token', { token });
        githubTokenInput.value = '';
        state.githubConnected = true;
        state.statusText = 'Saved.';
        state.statusIsUnsaved = false;
    } catch (e) {
        console.error('Failed to save GitHub token:', e);
        state.statusText = `Failed to connect: ${e}`;
        state.statusIsUnsaved = true;
    }
    state.githubBusy = false;
    render();
    clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
        state.statusText = '';
        render();
    }, 2200);
});

btnGithubDisconnect.addEventListener('click', async () => {
    try {
        await invoke('clear_github_token');
    } catch (e) {
        console.error('Failed to clear GitHub token:', e);
    }
    state.githubConnected = false;
    render();
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
        state.actionPermission = await invoke('get_action_permission');
    } catch (e) {
        console.error('Failed to load action permission:', e);
        state.actionPermission = 'off'; // fail closed, same as main.js
    }

    try {
        state.telemetryOn = await invoke('get_telemetry_enabled');
    } catch (e) {
        console.error('Failed to load telemetry-enabled setting:', e);
        state.telemetryOn = false; // fail closed
    }

    try {
        state.githubConnected = await invoke('get_github_token_status');
    } catch (e) {
        console.error('Failed to load GitHub connection status:', e);
    }

    for (const s of keySections) {
        await s.loadStatus();
    }

    try {
        const m = await invoke('get_model_settings');
        modelState.provider = m.provider;
        modelState.chosen = { gemini: m.gemini_model, openai: m.openai_model };
    } catch (e) {
        console.error('Failed to load model settings:', e);
    }
    renderModel();
    // Not awaited: a slow provider must not hold up the rest of Settings.
    loadModelList('gemini');
    loadModelList('openai');

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
