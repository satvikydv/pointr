import json
import re
from app.worker.celery_app import celery_app
from app.services.gemini import GeminiService
from app.services.session_memory import record_exchange
from app.config import settings


@celery_app.task(bind=True)
def run_agent_task(self, task_description: str, session_id: str, clipboard_text: str = ""):
    """Runs a text-only agent turn: task description plus whatever's on the
    user's clipboard (read client-side, since the worker has no OS access of
    its own). The model can hand back a replacement clipboard value; the
    client applies it after polling success, it's never written here."""
    print(f"[agent_task] clipboard_text len={len(clipboard_text)} preview={clipboard_text[:40]!r}")
    gemini = GeminiService(settings.gemini_api_key)

    clipboard_block = (
        f'The user\'s clipboard currently contains:\n"""\n{clipboard_text}\n"""\n'
        if clipboard_text
        else "The user's clipboard is currently empty.\n"
    )

    prompt = (
        "You are an agent completing a small task for the user, using their clipboard "
        "as context and optionally producing a new clipboard value. You have access to "
        "live Google Search — use it for anything time-sensitive, current, or outside "
        "what you'd otherwise know (news, prices, current versions, recent events, facts "
        "you're unsure of). Don't search for things you can already answer confidently.\n"
        f"{clipboard_block}"
        f"Task: {task_description}\n\n"
        "Format your answer EXACTLY as JSON, no markdown fences:\n"
        "{\n"
        '  "answer_text": "a short human-readable summary of what you did or found",\n'
        '  "clipboard_write": null\n'
        "  // clipboard_write is optional: a string to replace the clipboard with, or null/omitted to leave it untouched\n"
        "}"
    )

    raw = gemini.analyze_text_sync(prompt, use_search=True)

    try:
        cleaned = re.sub(r"```(?:json)?\n?", "", raw, flags=re.IGNORECASE).strip()
        parsed = json.loads(cleaned)
        answer_text = parsed.get("answer_text") or raw
        clipboard_write = parsed.get("clipboard_write")
        if not isinstance(clipboard_write, str):
            clipboard_write = None
    except Exception:
        answer_text = raw
        clipboard_write = None

    record_exchange(session_id, f"agent: {task_description}", answer_text)

    return {
        "status": "success",
        "result": answer_text,
        "pointer_target": None,
        "clipboard_write": clipboard_write,
    }
