const { invoke } = window.__TAURI__.core;
const { Window } = window.__TAURI__.window;

const runsList = document.getElementById('runs-list');
const emptyState = document.getElementById('empty-state');
const footerCount = document.getElementById('footer-count');
const btnClearHistory = document.getElementById('btn-clear-history');
const footerDefault = document.getElementById('footer-default');
const footerConfirm = document.getElementById('footer-confirm');
const btnCancelClear = document.getElementById('btn-cancel-clear');
const btnConfirmClear = document.getElementById('btn-confirm-clear');
const btnClose = document.getElementById('btn-close');

const state = {
    runs: [], // AgentTraceRecord[] from get_agent_history, newest first
    expandedId: null,
    confirmingClear: false,
};

// Real per-step action_type strings (from agent.py's _VALID_ACTION_TYPES) —
// not the design mock's shorthand ("open"/"type"/"key").
const STEP_ICONS = {
    click: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="M4 2L4 13L6.8 10.6L8.6 14.3L10.4 13.4L8.6 9.7L12.5 9.4L4 2Z" fill="currentColor"></path></svg>`,
    type_text: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="4.5" width="13" height="8" rx="1.5" stroke="currentColor" stroke-width="1.3"></rect><circle cx="4.2" cy="8" r="0.7" fill="currentColor"></circle><circle cx="6.8" cy="8" r="0.7" fill="currentColor"></circle><circle cx="9.4" cy="8" r="0.7" fill="currentColor"></circle><circle cx="12" cy="8" r="0.7" fill="currentColor"></circle><rect x="4" y="10" width="8" height="1.2" rx="0.6" fill="currentColor"></rect></svg>`,
    open_app: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" stroke-width="1.3"></rect><line x1="1.5" y1="5.5" x2="14.5" y2="5.5" stroke="currentColor" stroke-width="1.3"></line><circle cx="3.6" cy="4" r="0.55" fill="currentColor"></circle></svg>`,
    key_press: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><circle cx="5.2" cy="8" r="2.7" stroke="currentColor" stroke-width="1.3"></circle><line x1="7.7" y1="8" x2="14" y2="8" stroke="currentColor" stroke-width="1.3"></line><line x1="11.2" y1="8" x2="11.2" y2="10.2" stroke="currentColor" stroke-width="1.3"></line><line x1="13" y1="8" x2="13" y2="10.2" stroke="currentColor" stroke-width="1.3"></line></svg>`,
    done: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><circle cx="8" cy="8" r="6.2" stroke="currentColor" stroke-width="1.3"></circle><path d="M5.3 8.2L7.1 9.9L10.7 6.1" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"></path></svg>`,
    scroll: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><rect x="5.5" y="1.5" width="5" height="9" rx="2.5" stroke="currentColor" stroke-width="1.3"></rect><line x1="8" y1="3.8" x2="8" y2="5.8" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"></line><path d="M4 12.5L8 15L12 12.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"></path></svg>`,
    wait: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><circle cx="8" cy="8" r="6.2" stroke="currentColor" stroke-width="1.2"></circle><path d="M8 4.6V8L10.2 9.6" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"></path></svg>`,
    error: `<svg width="12" height="12" viewBox="0 0 16 16" fill="none"><path d="M8 1.5L15 13.5H1L8 1.5Z" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round"></path><line x1="8" y1="6.2" x2="8" y2="9.4" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"></line><circle cx="8" cy="11.4" r="0.8" fill="currentColor"></circle></svg>`,
};

const STEP_SUCCESS_ICON = `<svg width="13" height="13" viewBox="0 0 14 14"><circle cx="7" cy="7" r="7" fill="rgba(125,220,158,0.18)"></circle><path d="M4 7.2L6 9.2L10 5" stroke="#7ddc9e" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" fill="none"></path></svg>`;
const STEP_FAILED_ICON = `<svg width="13" height="13" viewBox="0 0 14 14"><circle cx="7" cy="7" r="7" fill="rgba(239,91,91,0.18)"></circle><path d="M4.5 4.5L9.5 9.5M9.5 4.5L4.5 9.5" stroke="#ef5b5b" stroke-width="1.5" stroke-linecap="round"></path></svg>`;

// stopReason (from history.rs's AgentTraceRecord) -> display bucket. Two
// distinct backend error reasons (screenshot vs. network) collapse into one
// "error" bucket here — the note text underneath still shows the specific
// finalAnswer message, so nothing's actually lost.
const STATUS_STYLE = {
    done: { label: 'Completed', dot: '#7ddc9e', text: 'rgba(125,220,158,0.95)', bg: 'rgba(125,220,158,0.12)' },
    // A run can end via the model's own "done" while one or more steps along
    // the way silently failed (executed:false / had an executionError) — the
    // model doesn't know that, so it isn't reflected in stopReason at all.
    // Caught for real: a run badged "Completed" whose Ctrl+S step had a
    // hard error and whose typed text never actually landed anywhere.
    done_with_errors: { label: 'Completed with errors', dot: '#f2b84b', text: 'rgba(242,184,75,0.95)', bg: 'rgba(242,184,75,0.12)' },
    max_steps: { label: 'Stopped early', dot: '#f2b84b', text: 'rgba(242,184,75,0.95)', bg: 'rgba(242,184,75,0.12)' },
    // Code-level repeat guard tripped (main.js) — the model proposed the
    // exact same action 3 times in a row with no visible change.
    stuck_repeating: { label: 'Stuck repeating', dot: '#f2b84b', text: 'rgba(242,184,75,0.95)', bg: 'rgba(242,184,75,0.12)' },
    aborted: { label: 'Cancelled', dot: 'rgba(255,255,255,0.45)', text: 'rgba(255,255,255,0.6)', bg: 'rgba(255,255,255,0.08)' },
    screenshot_error: { label: 'Error', dot: '#ef5b5b', text: 'rgba(239,91,91,0.95)', bg: 'rgba(239,91,91,0.14)' },
    network_error: { label: 'Error', dot: '#ef5b5b', text: 'rgba(239,91,91,0.95)', bg: 'rgba(239,91,91,0.14)' },
};
const DEFAULT_STATUS_STYLE = STATUS_STYLE.aborted;

function hadStepFailure(record) {
    return (record.steps || []).some((s) => s.executed === false || !!s.executionError);
}

// "done" and "done_with_errors" are the only two synthetic buckets not
// taken directly from stopReason — every other stopReason already tells
// the whole story on its own.
function statusKeyFor(record) {
    if (record.stopReason === 'done') return hadStepFailure(record) ? 'done_with_errors' : 'done';
    return record.stopReason;
}

const NOTE_STYLE = {
    warn: { bg: 'rgba(242,184,75,0.1)', border: 'rgba(242,184,75,0.3)', text: 'rgba(242,184,75,0.95)' },
    neutral: { bg: 'rgba(255,255,255,0.05)', border: 'rgba(255,255,255,0.12)', text: 'rgba(255,255,255,0.6)' },
    bad: { bg: 'rgba(239,91,91,0.1)', border: 'rgba(239,91,91,0.3)', text: 'rgba(239,91,91,0.95)' },
};

function escapeHtml(str) {
    return String(str ?? '').replace(/[&<>"']/g, (c) => ({
        '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    }[c]));
}

function formatRelativeTime(iso) {
    const then = new Date(iso).getTime();
    if (Number.isNaN(then)) return '';
    const diffSec = Math.max(0, (Date.now() - then) / 1000);
    if (diffSec < 60) return 'Just now';
    const diffMin = Math.floor(diffSec / 60);
    if (diffMin < 60) return `${diffMin}m ago`;
    const diffHr = Math.floor(diffMin / 60);
    if (diffHr < 24) return `${diffHr}h ago`;
    const diffDay = Math.floor(diffHr / 24);
    if (diffDay === 1) return 'Yesterday';
    if (diffDay < 30) return `${diffDay}d ago`;
    return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

function truncate(str, max) {
    const s = String(str ?? '').trim();
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

// Builds the note shown under an expanded run's step timeline. Wording is
// derived from the record's own stopReason/finalAnswer rather than any
// invented copy — MULTI_STEP_MAX_STEPS lives in main.js, so the wording
// here stays generic instead of hardcoding a step count that could drift.
function buildNote(record) {
    if (record.stopReason === 'done' && hadStepFailure(record)) {
        const failCount = (record.steps || []).filter((s) => s.executed === false || !!s.executionError).length;
        return {
            tone: 'warn',
            text: `The model reported this as done, but ${failCount} step${failCount === 1 ? '' : 's'} along the way failed to execute — worth checking the actual result.`,
        };
    }
    switch (record.stopReason) {
        case 'max_steps':
            return { tone: 'warn', text: 'Stopped early: reached the step limit before finishing.' };
        case 'stuck_repeating':
            return { tone: 'warn', text: record.finalAnswer || 'Stopped after repeating the same action with no visible change.' };
        case 'aborted':
            return { tone: 'neutral', text: 'Cancelled before finishing.' };
        case 'screenshot_error':
        case 'network_error':
            return { tone: 'bad', text: record.finalAnswer || 'Something went wrong mid-task.' };
        default:
            return null;
    }
}

function stepDesc(step) {
    if (step.description) return step.description;
    if (step.actionType === 'done') return step.answerText || 'Task complete';
    return step.actionType;
}

function renderStep(step, isLast) {
    const icon = STEP_ICONS[step.actionType] || STEP_ICONS.open_app;
    const ok = step.executed && !step.executionError;
    return `
        <div class="step-row" style="padding-bottom:${isLast ? 0 : 14}px;">
            <div class="step-icon-col">
                <div class="step-icon">${icon}</div>
                ${isLast ? '' : '<div class="step-line"></div>'}
            </div>
            <div class="step-body">
                <div class="step-desc">${escapeHtml(stepDesc(step))}</div>
                <div class="step-status">${ok ? STEP_SUCCESS_ICON : STEP_FAILED_ICON}</div>
            </div>
        </div>`;
}

function renderRun(record) {
    const isExpanded = state.expandedId === record.id;
    const style = STATUS_STYLE[statusKeyFor(record)] || DEFAULT_STATUS_STYLE;
    const preview = truncate(record.finalAnswer, 90) || '—';

    let detail = '';
    if (isExpanded) {
        const planHtml = (record.plan || []).map((text, i) => `
            <div class="plan-item">
                <div class="plan-num">${i + 1}</div>
                <div class="plan-text">${escapeHtml(text)}</div>
            </div>`).join('');

        const steps = record.steps || [];
        const stepsHtml = steps.map((s, i) => renderStep(s, i === steps.length - 1)).join('');

        const note = buildNote(record);
        const noteHtml = note
            ? (() => {
                const ns = NOTE_STYLE[note.tone];
                return `<div class="run-note" style="background:${ns.bg};border:1px solid ${ns.border};color:${ns.text};">${escapeHtml(note.text)}</div>`;
            })()
            : '';

        detail = `
            <div class="run-detail">
                <div class="detail-label">Confirmed plan</div>
                <div class="plan-box">${planHtml}</div>
                <div class="detail-label">Steps</div>
                <div>${stepsHtml}</div>
                ${noteHtml}
            </div>`;
    }

    return `
        <div class="run-row${isExpanded ? ' expanded' : ''}" data-run-id="${escapeHtml(record.id)}">
            <div class="run-head">
                <div style="flex:1;min-width:0;">
                    <div class="run-task">${escapeHtml(record.taskDescription)}</div>
                    <div class="run-meta">
                        <div class="run-time">${formatRelativeTime(record.startedAt)}</div>
                        <div class="run-sep">&middot;</div>
                        <div class="run-preview">${escapeHtml(preview)}</div>
                    </div>
                </div>
                <div class="run-side">
                    <div class="run-badge" style="background:${style.bg};">
                        <div class="run-badge-dot" style="background:${style.dot};"></div>
                        <div class="run-badge-text" style="color:${style.text};">${style.label}</div>
                    </div>
                    <div class="run-chevron"></div>
                </div>
            </div>
            ${detail}
        </div>`;
}

function render() {
    const hasRuns = state.runs.length > 0;
    emptyState.style.display = hasRuns ? 'none' : 'flex';
    runsList.style.display = hasRuns ? '' : 'none';
    runsList.innerHTML = hasRuns ? state.runs.map(renderRun).join('') : '';

    footerCount.textContent = `${state.runs.length} ${state.runs.length === 1 ? 'run logged' : 'runs logged'}`;
    btnClearHistory.style.display = hasRuns ? '' : 'none';

    if (state.confirmingClear) {
        footerDefault.classList.add('hidden');
        btnClearHistory.style.display = 'none';
        footerConfirm.classList.add('visible');
    } else {
        footerDefault.classList.remove('hidden');
        footerConfirm.classList.remove('visible');
    }
}

runsList.addEventListener('click', (e) => {
    const row = e.target.closest('.run-row');
    if (!row) return;
    const id = row.dataset.runId;
    state.expandedId = state.expandedId === id ? null : id;
    render();
});

btnClose.addEventListener('click', async () => {
    await Window.getCurrent().hide();
});

btnClearHistory.addEventListener('click', () => {
    state.confirmingClear = true;
    render();
});

btnCancelClear.addEventListener('click', () => {
    state.confirmingClear = false;
    render();
});

btnConfirmClear.addEventListener('click', async () => {
    try {
        await invoke('clear_agent_history');
    } catch (e) {
        console.error('Failed to clear agent history:', e);
    }
    state.runs = [];
    state.expandedId = null;
    state.confirmingClear = false;
    render();
});

async function loadHistory() {
    try {
        state.runs = await invoke('get_agent_history');
    } catch (e) {
        console.error('Failed to load agent history:', e);
        state.runs = [];
    }
    render();
}

// The window is shown/hidden repeatedly (tray menu), not recreated, so a
// fresh load on every focus is what keeps a newly-finished run visible
// without the user having to close and reopen.
window.addEventListener('focus', loadHistory);
loadHistory();
