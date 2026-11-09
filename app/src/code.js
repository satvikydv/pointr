// Code panel: streams a coding solution (Rust: meeting/code.rs) into
// sections with highlighted code. The window never takes focus or mouse
// input; it's driven by events and keys only.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const dot = document.getElementById('dot');
const langEl = document.getElementById('lang');
const timingEl = document.getElementById('timing');
const body = document.getElementById('body');
const solveKeyEl = document.getElementById('solve-key');

let currentId = 0;
let text = '';

function escapeHtml(s) {
    return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function inline(s) {
    return escapeHtml(s)
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<b>$1</b>');
}

// Fence tags the model might use, mapped to highlight.js language names.
const FENCE_ALIASES = { 'c++': 'cpp', 'c#': 'csharp', cs: 'csharp', js: 'javascript', ts: 'typescript', golang: 'go', py: 'python', kt: 'kotlin', rs: 'rust' };

function highlighted(code, tag) {
    const lang = FENCE_ALIASES[tag] || tag;
    if (window.hljs && lang && hljs.getLanguage(lang)) {
        try {
            return hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
        } catch (e) {
            console.error('highlight failed:', e);
        }
    }
    return escapeHtml(code);
}

// Code with a line-number gutter. The gutter is a separate column rather
// than per-line wrappers, because highlight.js spans can cross lines
// (multi-line strings and comments).
function codeBlock(lines, tag) {
    const code = lines.join('\n');
    const numbers = lines.map((_, i) => i + 1).join('\n');
    return `<div class="code"><div class="gutter">${numbers}</div><pre><code class="hljs">${highlighted(code, tag)}</code></pre></div>`;
}

// The route's format: "Language: X (why)", then ### sections with
// paragraphs, "- " bullets and one fenced code block. Works on a partial
// answer too (an unclosed fence while it streams).
function render(src) {
    const out = [];
    let bullets = [];
    let code = null;
    let tag = '';
    const flushBullets = () => {
        if (bullets.length) {
            out.push('<ul>' + bullets.map((b) => `<li>${inline(b)}</li>`).join('') + '</ul>');
            bullets = [];
        }
    };
    for (const raw of src.split('\n')) {
        const fence = raw.match(/^\s*```\s*([\w+#-]*)/);
        if (fence) {
            if (code) {
                out.push(codeBlock(code, tag));
                code = null;
            } else {
                flushBullets();
                code = [];
                tag = (fence[1] || '').toLowerCase();
            }
            continue;
        }
        if (code) {
            code.push(raw.replace(/\s+$/, ''));
            continue;
        }
        const line = raw.trim();
        if (!line) continue;
        const language = line.match(/^language:\s*(.+)$/i);
        if (language) {
            langEl.textContent = language[1];
            continue;
        }
        const heading = line.match(/^#{1,4}\s+(.+)$/);
        if (heading) {
            flushBullets();
            out.push(`<h3>${escapeHtml(heading[1])}</h3>`);
        } else if (/^[-*•]\s+/.test(line)) {
            bullets.push(line.replace(/^[-*•]\s+/, ''));
        } else {
            flushBullets();
            out.push(`<p class="p">${inline(line)}</p>`);
        }
    }
    flushBullets();
    if (code) out.push(codeBlock(code, tag));
    return out.join('');
}

listen('code-start', ({ payload }) => {
    currentId = payload.id;
    text = '';
    dot.className = 'working';
    langEl.textContent = payload.language;
    timingEl.textContent = payload.revising ? 'Revising…' : 'Solving…';
    body.innerHTML = '<div class="muted">Reading the screen…</div>';
    body.scrollTop = 0;
});

listen('code-chunk', ({ payload }) => {
    if (payload.id !== currentId) return;
    text += payload.text;
    // Scroll position is left alone: the Problem and Approach stay in view
    // while the code streams in below them.
    body.innerHTML = render(text);
});

listen('code-done', ({ payload }) => {
    if (payload.id !== currentId) return;
    text = payload.text || text;
    body.innerHTML = render(text) || '<div class="muted">No solution came back.</div>';
    dot.className = 'done';
    timingEl.textContent = `${(payload.total_ms / 1000).toFixed(1)}s`;
});

listen('code-error', ({ payload }) => {
    if (payload.id !== currentId) return;
    dot.className = 'error';
    timingEl.textContent = '';
    body.innerHTML = `<div class="error">${escapeHtml(payload.message)}</div>`;
});

listen('code-scroll', ({ payload }) => {
    body.scrollBy({ top: payload * Math.round(body.clientHeight * 0.7) });
});

listen('code-reset', () => {
    currentId = 0;
    text = '';
    dot.className = '';
    langEl.textContent = '';
    timingEl.textContent = '';
    body.innerHTML = '<div class="muted">Press the solve key when a coding problem is on screen.</div>';
});

function showKey(key) {
    solveKeyEl.textContent = key || 'The solve key';
}

listen('code-keys', ({ payload }) => showKey(payload.solve_key));

invoke('get_code_status')
    .then((status) => showKey(status.solve_key))
    .catch((e) => console.error('get_code_status failed:', e));
