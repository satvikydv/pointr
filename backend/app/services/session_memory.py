"""
Bounded in-memory history per session_id, so a follow-up question ("how do I
center this?") can be answered against what was just asked/answered rather
than treating every screenshot as a cold start. Rust decides session
boundaries (new session_id whenever the foreground app changes — see
SessionState in lib.rs); this module just keeps the last few exchanges for
whatever session_id it's handed.

Single-process, in-memory, no lock: FastAPI/uvicorn runs this as one asyncio
event loop, and every mutation below completes synchronously between awaits,
so there's no interleaving to race. Fine for a single local user; would need
real synchronization (or Redis) behind multiple workers.
"""

from collections import deque, OrderedDict
from typing import Deque, Dict

MAX_SESSIONS = 50
MAX_HISTORY_PER_SESSION = 10
ANSWER_PREVIEW_LEN = 160

_sessions: "OrderedDict[str, Deque[dict]]" = OrderedDict()


def get_history(session_id: str) -> Deque[dict]:
    if session_id in _sessions:
        _sessions.move_to_end(session_id)
        return _sessions[session_id]
    return deque(maxlen=MAX_HISTORY_PER_SESSION)


def record_exchange(session_id: str, query_text: str, answer_text: str) -> None:
    if session_id not in _sessions:
        if len(_sessions) >= MAX_SESSIONS:
            _sessions.popitem(last=False)  # evict the oldest session
        _sessions[session_id] = deque(maxlen=MAX_HISTORY_PER_SESSION)
    _sessions.move_to_end(session_id)
    _sessions[session_id].append({
        "query_text": query_text,
        "answer_text": answer_text[:ANSWER_PREVIEW_LEN],
    })


def format_duration(seconds: float) -> str:
    seconds = max(0, int(seconds))
    if seconds < 60:
        return "under a minute"
    minutes = seconds // 60
    if minutes < 60:
        return f"{minutes} minute{'s' if minutes != 1 else ''}"
    hours = minutes // 60
    return f"{hours} hour{'s' if hours != 1 else ''}"


def build_session_context_block(request) -> str:
    """Text block to fold into the prompt — app dwell time plus recent
    Q&A in this session. Empty string if there's nothing worth adding yet
    (brand new session, no prior exchanges)."""
    parts = []

    if request.session_duration_secs and request.session_duration_secs > 20 and request.app_name:
        parts.append(
            f"The user has been active in {request.app_name} for "
            f"{format_duration(request.session_duration_secs)} without switching apps."
        )

    history = get_history(request.session_id)
    if history:
        recent = list(history)[-5:]
        lines = "\n".join(
            f'- Asked: "{h["query_text"]}" — Answered: "{h["answer_text"]}"'
            for h in recent
        )
        parts.append(f"Recent exchanges earlier in this session:\n{lines}")

    return "\n".join(parts)
