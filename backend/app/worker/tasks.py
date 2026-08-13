import base64
import json
import re
from app.worker.celery_app import celery_app
from app.services.gemini import GeminiService
from app.services.mcp_filesystem import run_agent_turn_with_filesystem_sync
from app.services.github_mcp import run_agent_turn_with_github_sync
from app.services.tavily_mcp import run_agent_turn_with_web_search_sync
from app.services.session_memory import record_exchange
from app.config import settings


def _extract_json(raw: str) -> dict:
    cleaned = re.sub(r"```(?:json)?\n?", "", raw, flags=re.IGNORECASE).strip()
    return json.loads(cleaned)


# Shared between the normal (phase-1) prompt and the filesystem-enabled
# (phase-2) prompt — a task like "read my resume and answer this question on
# the form on screen" needs BOTH file access AND the action vocabulary in
# the SAME call to actually propose typing the answer in; phase 2 didn't
# know this vocabulary existed at all until this was factored out and reused.
ACTION_VOCAB_BLOCK = (
    "You can also propose ONE desktop action, if the task clearly calls for it:\n"
    '  - {"action_type": "type_text", "text": "...", "description": "..."} — types text into '
    "whatever input field the user currently has focused (e.g. drafting a reply in an already-open "
    "compose window, or filling in an answer on a form that's on screen). Only use this when it's "
    "clear the user has something focused ready to receive text.\n"
    '  - {"action_type": "open_app", "app_name": "...", "description": "..."} — opens an app via the '
    "Start menu search (e.g. \"notepad\", \"calculator\"). app_name should be the plain app name, not a path.\n"
    "The action does NOT run automatically — the user is shown your \"description\" and must explicitly "
    "confirm before anything happens, so make the description a clear, honest, one-sentence summary of "
    "exactly what will happen. Omit proposed_action entirely (null) for any task that's just a question "
    "or doesn't need a desktop action.\n"
)


def _validate_proposed_action(proposed_action):
    if not isinstance(proposed_action, dict):
        return None
    action_type = proposed_action.get("action_type")
    description = proposed_action.get("description")
    if action_type == "type_text" and isinstance(proposed_action.get("text"), str) and description:
        return {"action_type": "type_text", "text": proposed_action["text"], "description": description}
    if action_type == "open_app" and isinstance(proposed_action.get("app_name"), str) and description:
        return {"action_type": "open_app", "app_name": proposed_action["app_name"], "description": description}
    return None


@celery_app.task(bind=True)
def run_agent_task(
    self, task_description: str, session_id: str, clipboard_text: str = "",
    screenshot_base64: str = "", github_token: str = "",
):
    """Runs an agent turn: task description, whatever's on the user's
    clipboard, and — since M6 — the current screenshot, so tasks like "draft
    a reply to the message on screen" actually have something to look at
    instead of falling back to clipboard content as the only available
    context. The model can hand back a replacement clipboard value or a
    proposed desktop action; neither runs here — the client applies
    clipboard_write directly and gates proposed_action behind an explicit
    user confirmation first.

    Two-phase filesystem access: the first call below never has filesystem
    tools attached (they cost a subprocess spawn to even declare), so every
    task pays exactly the same cost as before this existed. Only when the
    model itself flags needs_filesystem=true — because the task clearly
    needs to read/list something on the user's Desktop that isn't already
    visible on screen — does a second call happen, this time through
    services.mcp_filesystem with real (read-only) tools attached."""
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

    fs_block = (
        "You do NOT have file access on this turn. If the task clearly requires reading or listing "
        "files on the user's Desktop (e.g. \"summarize the file X on my desktop\", \"find files mentioning "
        "Y\") and that information isn't already visible in the screenshot or clipboard, respond with "
        'ONLY this JSON instead of attempting to answer: {"needs_filesystem": true, "answer_text": '
        '"one short sentence on what file access you need"}. Set needs_filesystem to false or omit it '
        "entirely for every other task — most tasks don't need this.\n"
    )

    github_block = (
        "If the task requires looking at GitHub (issues, pull requests, repos, code, commits) and that "
        "information isn't already visible in the screenshot or clipboard, respond with ONLY this JSON "
        'instead: {"needs_github": true, "answer_text": "one short sentence on what you need to check"}. '
        "Only for tasks that clearly reference GitHub — most tasks don't need this, and needs_github/"
        "needs_filesystem/needs_multi_step are mutually exclusive (pick at most one).\n"
        if github_token
        else ""
    )

    web_search_block = (
        "If the task needs current/live information from the web (news, prices, facts outside your "
        "training data, anything time-sensitive) and that information isn't already visible in the "
        "screenshot or clipboard, respond with ONLY this JSON instead: "
        '{"needs_web_search": true, "answer_text": "one short sentence on what you\'re about to look up"}. '
        "Only for tasks that clearly need a live web lookup — most tasks don't, and needs_web_search/"
        "needs_filesystem/needs_github/needs_multi_step are mutually exclusive (pick at most one).\n"
        if settings.tavily_api_key
        else ""
    )

    multi_step_block = (
        "If the task genuinely requires MULTIPLE desktop actions in sequence (e.g. open an app then type "
        "into it, or click through several steps to navigate somewhere and search for something), respond "
        'with ONLY this JSON instead: {"needs_multi_step": true, "plan": ["short step 1", "short step 2", '
        '...], "answer_text": "one short sentence describing what you\'re about to do"}. Keep the plan to a '
        "handful of high-level steps, not exact clicks — it's a rough guide the user confirms once, not a "
        "precise script; the actual steps get worked out live against the real screen afterward. Only use "
        "this for tasks that truly need more than one action — a single type_text/open_app should still use "
        "proposed_action, and needs_multi_step/needs_filesystem are mutually exclusive (pick one).\n"
    )

    prompt = (
        "You are an agent completing a small task for the user, using their clipboard and current screen "
        "as context and optionally producing a new clipboard value.\n"
        f"{screen_block}"
        f"{fs_block}"
        f"{github_block}"
        f"{web_search_block}"
        f"{multi_step_block}"
        f"{ACTION_VOCAB_BLOCK}"
        f"{clipboard_block}"
        f"Task: {task_description}\n\n"
        "Format your answer EXACTLY as JSON, no markdown fences:\n"
        "{\n"
        '  "answer_text": "a short human-readable summary of what you did or found",\n'
        '  "clipboard_write": null,\n'
        '  "proposed_action": null,\n'
        '  "needs_filesystem": false,\n'
        '  "needs_github": false,\n'
        '  "needs_web_search": false,\n'
        '  "needs_multi_step": false\n'
        "  // clipboard_write is optional: a string to replace the clipboard with, or null/omitted to leave it untouched\n"
        "  // proposed_action is optional: one of the two action objects above, or null/omitted\n"
        "}"
    )

    # Decoded once, reused for both phase 1 (below) and phase 2 (the
    # filesystem-enabled call, further down) — a task needing both file
    # access and screen context (e.g. "read my resume and answer this
    # question on the form on screen") needs the same image both times.
    image_bytes = None
    if screenshot_base64:
        try:
            image_bytes = base64.b64decode(screenshot_base64)
        except Exception:
            import traceback
            traceback.print_exc()
            image_bytes = None

    if image_bytes:
        raw = gemini.analyze_sync(image_bytes, prompt)
    else:
        raw = gemini.analyze_text_sync(prompt)

    try:
        parsed = _extract_json(raw)
        needs_filesystem = bool(parsed.get("needs_filesystem"))
        needs_github = bool(parsed.get("needs_github")) and not needs_filesystem
        needs_web_search = (
            bool(parsed.get("needs_web_search")) and not needs_filesystem and not needs_github
        )
        needs_multi_step = (
            bool(parsed.get("needs_multi_step"))
            and not needs_filesystem and not needs_github and not needs_web_search
        )
        plan = parsed.get("plan")
        plan = [s for s in plan if isinstance(s, str)] if isinstance(plan, list) else []
        answer_text = parsed.get("answer_text") or raw
        clipboard_write = parsed.get("clipboard_write")
        if not isinstance(clipboard_write, str):
            clipboard_write = None
        proposed_action = _validate_proposed_action(parsed.get("proposed_action"))
    except Exception:
        needs_filesystem = False
        needs_github = False
        needs_web_search = False
        needs_multi_step = False
        plan = []
        answer_text = raw
        clipboard_write = None
        proposed_action = None

    if needs_filesystem:
        if not settings.pointr_fs_root:
            answer_text = (
                "File access isn't set up yet — set POINTR_FS_ROOT_HOST in the project's .env to a real "
                "folder and restart the worker (docker compose up -d --build worker)."
            )
            clipboard_write = None
            proposed_action = None
        else:
            fs_prompt = (
                f"You are an agent with read-only tool access to exactly one folder: '{settings.pointr_fs_root}' "
                "(the user's Desktop). Always pass that exact path as the tool argument — you already know "
                "it, don't call list_allowed_directories to look it up. Call list_directory on it AT MOST "
                "ONCE to see what's there; do NOT recurse into subdirectories or call any further tool "
                "unless the task explicitly names a specific file or subfolder to look inside. For .pdf "
                "files, use read_pdf_text, never read_text_file (which can't handle PDF's binary format). "
                "Use at most 2 file tool calls total, then work with what you already have — do not keep "
                "exploring.\n"
                f"{screen_block}"
                f"{ACTION_VOCAB_BLOCK}"
                f"{clipboard_block}"
                f"Task: {task_description}\n\n"
                "Format your FINAL answer EXACTLY as JSON, no markdown fences:\n"
                "{\n"
                '  "answer_text": "...",\n'
                '  "clipboard_write": null,\n'
                '  "proposed_action": null\n'
                "}"
            )
            try:
                raw2 = run_agent_turn_with_filesystem_sync(
                    fs_prompt, settings.gemini_model, settings.gemini_api_key, settings.pointr_fs_root,
                    image_bytes,
                )
                try:
                    parsed2 = _extract_json(raw2)
                    answer_text = parsed2.get("answer_text") or raw2
                    clipboard_write = parsed2.get("clipboard_write")
                    if not isinstance(clipboard_write, str):
                        clipboard_write = None
                    proposed_action = _validate_proposed_action(parsed2.get("proposed_action"))
                except Exception:
                    answer_text = raw2
                    clipboard_write = None
                    proposed_action = None
            except Exception:
                import traceback
                traceback.print_exc()
                answer_text = "Something went wrong reading files for this task."
                clipboard_write = None
                proposed_action = None

    if needs_github:
        if not github_token:
            answer_text = "GitHub isn't connected yet — add a token in Settings."
            clipboard_write = None
            proposed_action = None
        else:
            gh_prompt = (
                "You are an agent with read-only tool access to GitHub (issues, pull requests, repos, code, "
                "commits) via the user's own token — you can only see what their token can see. Use at most "
                "3 tool calls total, then work with what you already have.\n"
                f"{screen_block}"
                f"{ACTION_VOCAB_BLOCK}"
                f"{clipboard_block}"
                f"Task: {task_description}\n\n"
                "Format your FINAL answer EXACTLY as JSON, no markdown fences:\n"
                "{\n"
                '  "answer_text": "...",\n'
                '  "clipboard_write": null,\n'
                '  "proposed_action": null\n'
                "}"
            )
            try:
                raw3 = run_agent_turn_with_github_sync(
                    gh_prompt, settings.gemini_model, settings.gemini_api_key, github_token, image_bytes,
                )
                try:
                    parsed3 = _extract_json(raw3)
                    answer_text = parsed3.get("answer_text") or raw3
                    clipboard_write = parsed3.get("clipboard_write")
                    if not isinstance(clipboard_write, str):
                        clipboard_write = None
                    proposed_action = _validate_proposed_action(parsed3.get("proposed_action"))
                except Exception:
                    answer_text = raw3
                    clipboard_write = None
                    proposed_action = None
            except Exception:
                import traceback
                traceback.print_exc()
                answer_text = "Something went wrong checking GitHub for this task."
                clipboard_write = None
                proposed_action = None

    if needs_web_search:
        if not settings.tavily_api_key:
            answer_text = "Web search isn't set up yet — set TAVILY_API_KEY in the project's .env."
            clipboard_write = None
            proposed_action = None
        else:
            ws_prompt = (
                "You are an agent with access to live web search (tavily_search) and page extraction "
                "(tavily_extract) tools. Use at most 3 tool calls total, then work with what you already "
                "have — don't keep searching once you have enough to answer.\n"
                f"{screen_block}"
                f"{ACTION_VOCAB_BLOCK}"
                f"{clipboard_block}"
                f"Task: {task_description}\n\n"
                "Format your FINAL answer EXACTLY as JSON, no markdown fences:\n"
                "{\n"
                '  "answer_text": "...",\n'
                '  "clipboard_write": null,\n'
                '  "proposed_action": null\n'
                "}"
            )
            try:
                raw4 = run_agent_turn_with_web_search_sync(
                    ws_prompt, settings.gemini_model, settings.gemini_api_key, settings.tavily_api_key,
                    image_bytes,
                )
                try:
                    parsed4 = _extract_json(raw4)
                    answer_text = parsed4.get("answer_text") or raw4
                    clipboard_write = parsed4.get("clipboard_write")
                    if not isinstance(clipboard_write, str):
                        clipboard_write = None
                    proposed_action = _validate_proposed_action(parsed4.get("proposed_action"))
                except Exception:
                    answer_text = raw4
                    clipboard_write = None
                    proposed_action = None
            except Exception:
                import traceback
                traceback.print_exc()
                answer_text = "Something went wrong searching the web for this task."
                clipboard_write = None
                proposed_action = None

    # Multi-step tasks stop here — the plan is handed to the client to show
    # a one-time confirm, then it drives the actual step loop itself against
    # /api/agent/step (one action per fresh screenshot), not this task.
    multi_step_plan = plan if (needs_multi_step and plan) else None

    record_exchange(session_id, f"agent: {task_description}", answer_text)

    return {
        "status": "success",
        "result": answer_text,
        "pointer_target": None,
        "clipboard_write": clipboard_write,
        "proposed_action": proposed_action,
        "multi_step_plan": multi_step_plan,
    }
