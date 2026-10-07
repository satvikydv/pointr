"""
Short conversation memory, so a follow-up ("make it shorter", "and the
second one?", "now open that in Notepad") is answered against what was just
asked, answered, explained or done instead of treating every capture as a
cold start.

Lives in Redis, not process memory, because the API process (direct
questions, explain) and the Celery worker (agent tasks) are separate
processes: with per-process memory an agent task could never see the question
that came before it, and a question never saw the agent's draft. Redis here
persists nothing to disk (see docker-compose.prod.yml), so this is still
memory-only: it is cleared when Redis restarts and every conversation expires
on its own after TTL_SECS of inactivity.

Keyed by the desktop client's conversation thread (it follows the user across
apps for a few minutes), falling back to the per-app session id from older
clients. Memory is strictly best-effort: if Redis is unreachable the request
carries on with no history, it never fails because of it.
"""

import json
import logging
from typing import Optional

from redis import Redis
from redis.asyncio import Redis as AsyncRedis

from app.config import settings

log = logging.getLogger(__name__)

MAX_EXCHANGES = 12  # kept per conversation
CONTEXT_EXCHANGES = 6  # folded into a prompt
TTL_SECS = 20 * 60  # idle expiry, refreshed on every write
STORED_QUERY_LEN = 500
STORED_ANSWER_LEN = 1500
# The newest exchanges carry the most weight for "it"/"that" follow-ups, so
# the last two keep a long answer (a drafted reply has to survive intact for
# "make it shorter"); older ones are only there for the thread of the topic.
RECENT_FULL = 2
RECENT_ANSWER_LEN = 700
OLDER_ANSWER_LEN = 160

_MODE_LABELS = {
    "direct": "Asked",
    "explain": "Asked for an explanation of",
    "agent": "Gave an agent task",
    "steps": "Ran an on-screen task",
}

_sync_client: Optional[Redis] = None
_async_client: Optional[AsyncRedis] = None


def _get_sync() -> Redis:
    global _sync_client
    if _sync_client is None:
        _sync_client = Redis.from_url(settings.redis_url, decode_responses=True)
    return _sync_client


def _get_async() -> AsyncRedis:
    global _async_client
    if _async_client is None:
        _async_client = AsyncRedis.from_url(settings.redis_url, decode_responses=True)
    return _async_client


def memory_key(thread_id: str, session_id: str) -> str:
    return f"mem:{thread_id or session_id}"


def _entry(mode: str, query: str, answer: str) -> str:
    return json.dumps(
        {
            "mode": mode if mode in _MODE_LABELS else "direct",
            "query": (query or "")[:STORED_QUERY_LEN],
            "answer": (answer or "")[:STORED_ANSWER_LEN],
        }
    )


def _decode(raw_items) -> list[dict]:
    out = []
    for raw in raw_items or []:
        try:
            item = json.loads(raw)
        except Exception:
            continue
        if isinstance(item, dict):
            out.append(item)
    return out


def record_exchange(thread_id: str, session_id: str, mode: str, query: str, answer: str) -> None:
    """Sync variant, for the Celery worker."""
    key = memory_key(thread_id, session_id)
    try:
        r = _get_sync()
        r.rpush(key, _entry(mode, query, answer))
        r.ltrim(key, -MAX_EXCHANGES, -1)
        r.expire(key, TTL_SECS)
    except Exception as e:
        log.warning("conversation memory write failed: %s", type(e).__name__)


async def arecord_exchange(thread_id: str, session_id: str, mode: str, query: str, answer: str) -> None:
    key = memory_key(thread_id, session_id)
    try:
        r = _get_async()
        await r.rpush(key, _entry(mode, query, answer))
        await r.ltrim(key, -MAX_EXCHANGES, -1)
        await r.expire(key, TTL_SECS)
    except Exception as e:
        log.warning("conversation memory write failed: %s", type(e).__name__)


def get_history(thread_id: str, session_id: str) -> list[dict]:
    try:
        return _decode(_get_sync().lrange(memory_key(thread_id, session_id), -CONTEXT_EXCHANGES, -1))
    except Exception as e:
        log.warning("conversation memory read failed: %s", type(e).__name__)
        return []


async def aget_history(thread_id: str, session_id: str) -> list[dict]:
    try:
        raw = await _get_async().lrange(memory_key(thread_id, session_id), -CONTEXT_EXCHANGES, -1)
        return _decode(raw)
    except Exception as e:
        log.warning("conversation memory read failed: %s", type(e).__name__)
        return []


def _clip(text: str, limit: int) -> str:
    text = " ".join((text or "").split())
    return text if len(text) <= limit else text[: limit - 1].rstrip() + "…"


def format_history(history: list[dict]) -> str:
    """The earlier exchanges as a prompt block, oldest first. Empty string
    when there is nothing yet."""
    if not history:
        return ""
    lines = []
    last = len(history) - 1
    for i, h in enumerate(history):
        recent = i > last - RECENT_FULL
        label = _MODE_LABELS.get(h.get("mode"), "Asked")
        answer = _clip(h.get("answer", ""), RECENT_ANSWER_LEN if recent else OLDER_ANSWER_LEN)
        lines.append(f'- {label}: "{_clip(h.get("query", ""), 200)}" - Result: "{answer}"')
    return (
        "Conversation so far, oldest first. A short follow-up such as \"make it shorter\", "
        "\"what about the second one\" or \"now do that in Notepad\" refers to these; "
        "ignore them if the new request is clearly about something else:\n" + "\n".join(lines)
    )


def format_duration(seconds: float) -> str:
    seconds = max(0, int(seconds))
    if seconds < 60:
        return "under a minute"
    minutes = seconds // 60
    if minutes < 60:
        return f"{minutes} minute{'s' if minutes != 1 else ''}"
    hours = minutes // 60
    return f"{hours} hour{'s' if hours != 1 else ''}"


def _dwell_line(request) -> str:
    if getattr(request, "session_duration_secs", 0) and request.session_duration_secs > 20 and getattr(request, "app_name", ""):
        return (
            f"The user has been active in {request.app_name} for "
            f"{format_duration(request.session_duration_secs)} without switching apps."
        )
    return ""


def compose_context(request, history: list[dict]) -> str:
    """Prompt block: app dwell time, what the client says is going on right
    now (extra_context, e.g. the explanation the user just interrupted), and
    the recent conversation. Empty when there is nothing worth adding."""
    parts = [
        _dwell_line(request),
        (getattr(request, "extra_context", "") or "").strip(),
        format_history(history),
    ]
    return "\n".join(p for p in parts if p)


async def build_session_context_block(request) -> str:
    history = await aget_history(getattr(request, "thread_id", ""), request.session_id)
    return compose_context(request, history)
