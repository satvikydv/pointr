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

// `code` spans, and **bold** in case the model adds it anyway (shown as
// plain emphasis, never as literal asterisks).
function inline(s) {
    return escapeHtml(s)
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>');
}

// The route's format: "Q: ..." line, answer sentences, "- " bullets, and
// for coding questions one ``` fenced block. Works on a partial answer
// too (an unclosed fence while it streams).
function renderAnswer(text) {
    const out = [];
    let bullets = [];
    let code = null; // lines of the open fenced block
    const flushBullets = () => {
        if (bullets.length) {
            out.push('<ul>' + bullets.map((b) => `<li>${inline(b)}</li>`).join('') + '</ul>');
            bullets = [];
        }
    };
    const flushCode = () => {
        out.push(`<pre><code>${escapeHtml(code.join('\n'))}</code></pre>`);
        code = null;
    };
    for (const raw of text.split('\n')) {
        if (/^\s*```/.test(raw)) {
            if (code) {
                flushCode();
            } else {
                flushBullets();
                code = [];
            }
            continue;
        }
        if (code) {
            code.push(raw.replace(/\s+$/, '')); // keep indentation
            continue;
        }
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
    if (code) flushCode();
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
        // Every candidate is Ctrl+Alt+<letter>, and the footer says Ctrl+Alt is held.
        document.getElementById('end-key').textContent = status.toggle_key ? status.toggle_key.split('+').pop() : 'tray menu';
        if (status.active) setState('listening', 'Listening');
    })
    .catch((e) => console.error('get_meeting_status failed:', e));

// ---------------------------------------------------------------------
// Coding solutions (meeting/code.rs): the code itself goes to the code
// panel; this panel mirrors the Problem and Approach so the user can start
// talking. Its own listeners, so the answer handlers above are untouched.
// Whichever started last (an answer or a solve) owns this panel's body.
// ---------------------------------------------------------------------

let codeId = 0;
let codeText = '';
let showingCode = false;

listen('meeting-answer-start', () => {
    showingCode = false;
});

// "### Problem" and "### Approach" from a (possibly partial) solution, in
// this panel's answer format: a Q line, then sentences.
function codeSummary(src) {
    const out = [];
    let section = '';
    for (const raw of src.split('\n')) {
        const line = raw.trim();
        const heading = line.match(/^#{1,4}\s+(.+)$/);
        if (heading) {
            section = heading[1].toLowerCase();
            if (section.startsWith('code')) break;
            continue;
        }
        if (!line || /^language:/i.test(line)) continue;
        if (section.startsWith('problem')) out.push(`Q: ${line}`);
        else if (section.startsWith('approach')) out.push(line);
    }
    return out.join('\n');
}

function renderCodeMirror(note) {
    if (!showingCode) return;
    const summary = renderAnswer(codeSummary(codeText));
    body.innerHTML = (summary || '<div class="muted">Reading the coding question on screen…</div>') + `<div class="muted">${note}</div>`;
}

listen('code-start', ({ payload }) => {
    codeId = payload.id;
    codeText = '';
    showingCode = true;
    latencyEl.textContent = '';
    renderCodeMirror(payload.revising ? 'Revising the solution → code panel' : 'Solving → code panel');
});

listen('code-chunk', ({ payload }) => {
    if (payload.id !== codeId) return;
    codeText += payload.text;
    renderCodeMirror('Code → code panel');
});

listen('code-done', ({ payload }) => {
    if (payload.id !== codeId) return;
    codeText = payload.text || codeText;
    renderCodeMirror('Full solution in the code panel');
});

listen('code-error', ({ payload }) => {
    if (payload.id !== codeId || !showingCode) return;
    showMessage(`<div class="error">${escapeHtml(payload.message)}</div>`);
});
