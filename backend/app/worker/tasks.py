import base64
import json
import re
from app.worker.celery_app import celery_app
from app.services.gemini import GeminiService
from app.services.session_memory import record_exchange
from app.config import settings


@celery_app.task(bind=True)
def run_agent_task(self, task_description: str, session_id: str, clipboard_text: str = "", screenshot_base64: str = ""):
    """Runs an agent turn: task description, whatever's on the user's
    clipboard, and — since M6 — the current screenshot, so tasks like "draft
    a reply to the message on screen" actually have something to look at
    instead of falling back to clipboard content as the only available
    context (the bug that motivated adding this: agent tasks were text-only
    while the normal Q&A path was always vision-capable). The model can hand
    back a replacement clipboard value or a proposed desktop action; neither
    runs here — the client applies clipboard_write directly and gates
    proposed_action behind an explicit user confirmation first."""
    print(f"[agent_task] clipboard_text len={len(clipboard_text)} preview={clipboard_text[:40]!r} has_screenshot={bool(screenshot_base64)}")
    gemini = GeminiService(settings.gemini_api_key)

    clipboard_block = (
        f'The user\'s clipboard currently contains:\n"""\n{clipboard_text}\n"""\n'
        if clipboard_text
        else "The user's clipboard is currently empty.\n"
    )

    screen_block = (
        "You can see the user's current screen in the attached image — use it as context whenever the task "
        "refers to something on screen (e.g. \"reply to this message\", \"summarize what's open\"). Don't "
        "assume the clipboard is what the task is about just because it's the only text context available; "
        "the screenshot is usually the more relevant source for anything screen-referential.\n"
        if screenshot_base64
        else ""
    )

    prompt = (
        "You are an agent completing a small task for the user, using their clipboard and current screen "
        "as context and optionally producing a new clipboard value. You have access to "
        "live Google Search — use it for anything time-sensitive, current, or outside "
        "what you'd otherwise know (news, prices, current versions, recent events, facts "
        "you're unsure of). Don't search for things you can already answer confidently.\n"
        f"{screen_block}"
        "You can also propose ONE desktop action, if the task clearly calls for it:\n"
        '  - {"action_type": "type_text", "text": "...", "description": "..."} — types text into '
        "whatever input field the user currently has focused (e.g. drafting a reply in an already-open "
        "compose window). Only use this when it's clear the user has something focused ready to receive text.\n"
        '  - {"action_type": "open_app", "app_name": "...", "description": "..."} — opens an app via the '
        "Start menu search (e.g. \"notepad\", \"calculator\"). app_name should be the plain app name, not a path.\n"
        "The action does NOT run automatically — the user is shown your \"description\" and must explicitly "
        "confirm before anything happens, so make the description a clear, honest, one-sentence summary of "
        "exactly what will happen. Omit proposed_action entirely (null) for any task that's just a question "
        "or doesn't need a desktop action.\n"
        f"{clipboard_block}"
        f"Task: {task_description}\n\n"
        "Format your answer EXACTLY as JSON, no markdown fences:\n"
        "{\n"
        '  "answer_text": "a short human-readable summary of what you did or found",\n'
        '  "clipboard_write": null,\n'
        '  "proposed_action": null\n'
        "  // clipboard_write is optional: a string to replace the clipboard with, or null/omitted to leave it untouched\n"
        "  // proposed_action is optional: one of the two action objects above, or null/omitted\n"
        "}"
    )

    if screenshot_base64:
        try:
            image_bytes = base64.b64decode(screenshot_base64)
            raw = gemini.analyze_sync(image_bytes, prompt, use_search=True)
        except Exception:
            import traceback
            traceback.print_exc()
            raw = gemini.analyze_text_sync(prompt, use_search=True)
    else:
        raw = gemini.analyze_text_sync(prompt, use_search=True)

    try:
        cleaned = re.sub(r"```(?:json)?\n?", "", raw, flags=re.IGNORECASE).strip()
        parsed = json.loads(cleaned)
        answer_text = parsed.get("answer_text") or raw
        clipboard_write = parsed.get("clipboard_write")
        if not isinstance(clipboard_write, str):
            clipboard_write = None

        proposed_action = parsed.get("proposed_action")
        if isinstance(proposed_action, dict):
            action_type = proposed_action.get("action_type")
            description = proposed_action.get("description")
            if action_type == "type_text" and isinstance(proposed_action.get("text"), str) and description:
                proposed_action = {"action_type": "type_text", "text": proposed_action["text"], "description": description}
            elif action_type == "open_app" and isinstance(proposed_action.get("app_name"), str) and description:
                proposed_action = {"action_type": "open_app", "app_name": proposed_action["app_name"], "description": description}
            else:
                proposed_action = None
        else:
            proposed_action = None
    except Exception:
        answer_text = raw
        clipboard_write = None
        proposed_action = None

    record_exchange(session_id, f"agent: {task_description}", answer_text)

    return {
        "status": "success",
        "result": answer_text,
        "pointer_target": None,
        "clipboard_write": clipboard_write,
        "proposed_action": proposed_action,
    }
