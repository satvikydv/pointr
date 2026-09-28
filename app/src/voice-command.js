// Turns a push-to-talk transcript into a query for runDirectAnalysis.
//
// Spoken commands can't type "agent:" or "explain:", so the mode comes from
// a leading keyword. Speech doesn't start cleanly, though: people say "uh",
// "hey", "okay so" first, and the transcriber sometimes turns those into
// other words. Measured with Parakeet across US, UK and Indian English
// voices: "um" came back as "I'm" in 8 of 13 clips, once fused with the
// keyword itself ("Armagent"), and "ah"/"hey pointer" preceded it too. The
// keyword word itself was transcribed correctly every time in those clips;
// the mishear list below is a conservative cover for real voices.
//
// Only leading words are skipped, and only to find a keyword. A plain
// question keeps every word it was spoken with.

const FILLERS = new Set([
    'uh', 'uhh', 'um', 'umm', 'uhm', 'erm', 'er', 'ah', 'ahh', 'oh', 'hm', 'hmm', 'mm',
    "i'm", 'im', // "um" misheard
    'hey', 'hi', 'hello', 'okay', 'ok', 'so', 'well', 'alright', 'right', 'now',
    'yeah', 'yes', 'please', 'just', 'pointer', 'pointr', 'pointers',
]);

// Multi-word lead-ins, as normalised token sequences.
const FILLER_PHRASES = [
    ['can', 'you'], ['could', 'you'], ['would', 'you'], ['will', 'you'],
    ['i', 'want', 'you', 'to'], ['go', 'ahead', 'and'], ['all', 'right'],
];

const KEYWORDS = {
    agent: new Set(['agent', 'agents', "agent's", 'agentic', 'ajent', 'eigent']),
    explain: new Set(['explain', 'explains', 'explained', 'explaining', 'explane', 'xplain']),
};

// The keyword split in two by the transcriber.
const SPLIT_KEYWORDS = {
    'a gent': 'agent',
    'ex plain': 'explain',
    'x plain': 'explain',
};

// A filler fused onto the front of the keyword ("Armagent").
const FUSED = /^(?:um|uh|ah|arm|erm|im|hm+)(agent|explain)$/;

// Skip at most this many leading words looking for the keyword, so a
// keyword deep in an ordinary sentence never changes its mode.
const MAX_LEAD_WORDS = 6;

function norm(word) {
    return word.toLowerCase().replace(/[’`]/g, "'").replace(/^[^a-z0-9']+|[^a-z0-9']+$/g, '');
}

function keywordAt(words, i) {
    const w = words[i];
    for (const [mode, forms] of Object.entries(KEYWORDS)) {
        if (forms.has(w)) return { mode, length: 1 };
    }
    const fused = FUSED.exec(w || '');
    if (fused) return { mode: fused[1], length: 1 };
    if (i + 1 < words.length) {
        const pair = SPLIT_KEYWORDS[`${w} ${words[i + 1]}`];
        if (pair) return { mode: pair, length: 2 };
    }
    return null;
}

function fillerLength(words, i) {
    for (const phrase of FILLER_PHRASES) {
        if (phrase.every((p, k) => words[i + k] === p)) return phrase.length;
    }
    return FILLERS.has(words[i]) ? 1 : 0;
}

/// { mode: 'agent' | 'explain' | null, rest } — `rest` is what follows the
/// keyword (original casing, leading punctuation trimmed), or the whole
/// transcript unchanged when there's no keyword.
export function parseVoiceCommand(text) {
    const original = (text || '').trim().split(/\s+/).filter(Boolean);
    const words = original.map(norm);

    let i = 0;
    while (i < words.length && i < MAX_LEAD_WORDS) {
        const kw = keywordAt(words, i);
        if (kw) {
            const rest = original.slice(i + kw.length).join(' ').replace(/^[\s,.:;!?-]+/, '');
            // "agent" alone (or "agent?") isn't a task; leave it as a question.
            if (kw.mode === 'agent' && !rest) break;
            return { mode: kw.mode, rest };
        }
        const skip = fillerLength(words, i);
        if (!skip) break;
        i += skip;
    }
    return { mode: null, rest: (text || '').trim() };
}

/// The string runDirectAnalysis expects: "agent: …", "explain: …", or the
/// question as spoken.
export function voiceToQuery(text) {
    const { mode, rest } = parseVoiceCommand(text);
    return mode ? `${mode}: ${rest}` : rest;
}
