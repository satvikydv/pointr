// Meeting panel: shows meeting mode's state and streams answers in.
// Driven entirely by events from Rust (meeting/mod.rs, meeting/answer.rs);
// the window never takes focus or mouse input, so there's nothing to click.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const dot = document.getElementById('dot');
const stateEl = document.getElementById('state');
const heardEl = document.getElementById('heard');
const shieldEl = document.getElementById('shield');
const body = document.getElementById('body');
const latencyEl = document.getElementById('latency');

let currentId = 0;
let answerText = '';

function escapeHtml(s) {
    return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// [placeholder]s the model leaves for details it doesn't know.
function inline(s) {
    return escapeHtml(s).replace(/\[([^\]]{1,60})\]/g, '<span class="ph">[$1]</span>');
}

// The route's format: "Q: ..." line, answer sentences, "- " bullets.
function renderAnswer(text) {
    const out = [];
    let bullets = [];
    const flushBullets = () => {
        if (bullets.length) {
            out.push('<ul>' + bullets.map((b) => `<li>${inline(b)}</li>`).join('') + '</ul>');
            bullets = [];
        }
    };
    for (const raw of text.split('\n')) {
        const line = raw.trim();
        if (!line) continue;
        if (/^q:/i.test(line)) {
            flushBullets();
            out.push(`<div class="q">${inline(line.slice(2).trim())}</div>`);
        } else if (/^[-*•]\s+/.test(line)) {
            bullets.push(line.replace(/^[-*•]\s+/, ''));
        } else {
            flushBullets();
            out.push(`<p class="p">${inline(line)}</p>`);
        }
    }
    flushBullets();
    return out.join('');
}

function setState(kind, label) {
    dot.className = kind;
    stateEl.textContent = label;
}

function showMessage(html) {
    body.innerHTML = html;
    body.scrollTop = 0;
}

function showProtection(protection) {
    if (!protection || protection === 'hidden') {
        shieldEl.style.display = 'none';
        return;
    }
    shieldEl.style.display = 'block';
    shieldEl.textContent = protection === 'black_box'
        ? 'This Windows version can only black out this panel in screen shares, not hide it.'
        : 'This panel could not be hidden from screen sharing. Don\u2019t share your whole screen.';
}

listen('meeting-state', ({ payload }) => {
    const { state, message } = payload;
    if (state === 'listening') {
        setState(message ? 'warn' : 'listening', 'Listening');
        if (message) heardEl.textContent = message;
    } else if (state === 'reconnecting') {
        setState('warn', 'Reconnecting audio');
        heardEl.textContent = message || '';
    } else if (state === 'error') {
        setState('error', 'Not listening');
        heardEl.textContent = '';
        showMessage(`<div class="error">${escapeHtml(message || 'Meeting mode stopped.')}</div>`);
    } else if (state === 'stopped') {
        setState('', 'Meeting mode');
        heardEl.textContent = '';
        latencyEl.textContent = '';
        currentId = 0;
        answerText = '';
        showMessage('<div class="muted">Listening to the meeting audio. Press Ctrl+Alt+Enter when you want an answer to what was just asked.</div>');
    }
});

let lastLevelAt = 0;
listen('meeting-level', ({ payload }) => {
    const now = performance.now();
    if (now - lastLevelAt < 90) return;
    lastLevelAt = now;
    const scale = 1 + Math.min(1, payload.level * 12) * 0.8;
    dot.style.transform = `scale(${scale.toFixed(2)})`;
});

listen('meeting-heard', ({ payload }) => {
    heardEl.textContent = `\u201c${payload.text}\u201d`;
});

listen('meeting-answer-start', ({ payload }) => {
    currentId = payload.id;
    answerText = '';
    latencyEl.textContent = '';
    showMessage('<div class="muted">Thinking\u2026</div>');
});

listen('meeting-answer-chunk', ({ payload }) => {
    if (payload.id !== currentId) return;
    // Scroll position is left alone, so the question and first lines stay
    // in view while it streams; Ctrl+Alt+Down scrolls on.
    answerText += payload.text;
    body.innerHTML = renderAnswer(answerText);
});

listen('meeting-answer-done', ({ payload }) => {
    if (payload.id !== currentId) return;
    answerText = payload.answer_text || answerText;
    body.innerHTML = renderAnswer(answerText) || '<div class="muted">No answer came back.</div>';
    if (payload.first_token_ms != null) {
        latencyEl.textContent = `${(payload.first_token_ms / 1000).toFixed(1)}s`;
    }
});

listen('meeting-answer-error', ({ payload }) => {
    if (payload.id !== currentId) return;
    showMessage(`<div class="error">${escapeHtml(payload.message)}</div>`);
});

listen('meeting-scroll', ({ payload }) => {
    body.scrollBy({ top: payload * Math.round(body.clientHeight * 0.6) });
});

invoke('get_meeting_status')
    .then((status) => {
        showProtection(status.protection);
        document.getElementById('end-key').textContent = status.toggle_key || 'tray menu';
        if (status.active) setState('listening', 'Listening');
    })
    .catch((e) => console.error('get_meeting_status failed:', e));
