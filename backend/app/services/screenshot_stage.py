"""Screenshots uploaded ahead of the question they belong to.

The desktop app captures the screen the moment the hotkey is pressed, then
waits for the user to type or speak. Uploading the screenshot during that
wait, instead of with the question, takes a 1-3 MB upload off the critical
path: the question itself then carries only a short reference.

Held in this process's memory only (never written to disk, never in
Redis), for at most TTL_SECONDS, and bounded in total size so a flood of
uploads evicts old entries instead of exhausting memory. A reference that
has expired or been evicted is reported as such, and the client falls back
to sending the screenshot inline.
"""
import secrets
import threading
import time
from collections import OrderedDict

TTL_SECONDS = 60
MAX_TOTAL_BYTES = 64 * 1024 * 1024

_lock = threading.Lock()
# ref -> (screenshot_base64, expires_at); insertion order = age.
_entries: "OrderedDict[str, tuple[str, float]]" = OrderedDict()
_total = 0


def _drop(ref: str) -> None:
    global _total
    b64, _ = _entries.pop(ref)
    _total -= len(b64)


def _purge_expired(now: float) -> None:
    while _entries:
        ref, (_, expires_at) = next(iter(_entries.items()))
        if expires_at > now:
            break
        _drop(ref)


def stage(screenshot_base64: str) -> str:
    global _total
    ref = secrets.token_urlsafe(18)
    now = time.monotonic()
    with _lock:
        _purge_expired(now)
        while _entries and _total + len(screenshot_base64) > MAX_TOTAL_BYTES:
            _drop(next(iter(_entries)))
        _entries[ref] = (screenshot_base64, now + TTL_SECONDS)
        _total += len(screenshot_base64)
    return ref


def get(ref: str) -> str | None:
    """The staged screenshot, or None if unknown or expired. Not removed on
    read: a retried request can use the same reference; expiry cleans up."""
    now = time.monotonic()
    with _lock:
        _purge_expired(now)
        entry = _entries.get(ref)
        return entry[0] if entry else None


def discard(ref: str) -> None:
    with _lock:
        if ref in _entries:
            _drop(ref)
