import base64
import json
import re
from fastapi import APIRouter, Depends
from celery.result import AsyncResult
from app.models.agent import (
    AgentTaskRequest, AgentTaskResponse, AgentTaskStatusResponse,
    AgentStepRequest, AgentStepResponse,
)
from app.worker.tasks import run_agent_task
from app.services.gemini import GeminiService
from app.config import settings
from app.rate_limit import rate_limit

router = APIRouter()

_VALID_ACTION_TYPES = {"click", "type_text", "open_app", "key_press", "scroll", "wait", "done"}


def _build_step_prompt(task_description: str, plan: list, completed_steps: list, stuck: bool = False) -> str:
    plan_block = "\n".join(f"{i+1}. {s}" for i, s in enumerate(plan)) if plan else "(no plan given)"
    completed_block = "\n".join(f"- {s}" for s in completed_steps) if completed_steps else "(none yet)"
    # Prepended, not buried in the general instructions further down — the
    # client only sets this when it's DETECTED (by comparing real action
    # parameters, not free-text descriptions) that the exact same action is
    # about to be proposed a 2nd time in a row. A general "don't repeat"
    # instruction lower in this prompt already exists and isn't reliably
    # followed on its own (confirmed for real, repeatedly); this is a
    # one-shot, impossible-to-miss correction for that specific moment.
    stuck_block = (
        "STOP AND RECONSIDER: the action you're about to propose is IDENTICAL (same type, same text/point/key) "
        "to the one you just did last call, and the screen shows no visible change from it — it did not work. "
        "Do NOT propose that same action again. If your most recent completed step was open_app, the field "
        "you want to type into almost certainly does NOT have real keyboard focus yet — your next action MUST "
        "be a click at the exact point in the screenshot where that field actually is (look carefully — for a "
        "browser this is the address/search bar near the top, not the page body). If you already clicked and "
        "it still isn't working, try a different approach entirely rather than repeating.\n\n"
        if stuck
        else ""
    )
    return (
        "You are executing a multi-step desktop automation task, ONE action at a time. You'll be called "
        "again after each action with a fresh screenshot, so don't try to plan ahead — just decide the "
        "single best next action given what's actually on screen right now.\n\n"
        f"{stuck_block}"
        f"Overall task: {task_description}\n\n"
        f"Rough plan (a guide, not a script — deviate from it if the screen shows something different):\n"
        f"{plan_block}\n\n"
        f"Steps completed so far:\n{completed_block}\n\n"
        "Look at the attached screenshot and decide ONE next action, formatted as one of:\n"
        '  {"action_type": "click", "point": [y, x], "button": "left", "double": false, "description": "..."} '
        "— click a screen location. point is [y, x] normalized 0-1000 (Gemini's native grounding format — NOT "
        "0-1 fractions, NOT pixels, y BEFORE x). Aim for the center of the actual clickable element. button is "
        "\"left\" (default — omit it for a normal click) or \"right\" for a context menu. double is true only "
        "for double-click-to-open (e.g. a file/folder icon); omit or false otherwise.\n"
        '  {"action_type": "type_text", "text": "...", "description": "..."} — types into whatever is '
        "currently focused. Click the target field first (as a separate step) if nothing is focused yet.\n"
        '  {"action_type": "key_press", "key": "Enter", "description": "..."} — presses a key, or a modifier '
        "combo. Bare key: Enter, Tab, Escape, Backspace, Delete, ArrowDown, ArrowUp, ArrowLeft, ArrowRight, or "
        "any single letter/digit. Combo: modifiers joined with \"+\", the real key last — e.g. \"Ctrl+S\" "
        "(save), \"Ctrl+C\" (copy), \"Ctrl+V\" (paste), \"Ctrl+A\" (select all), \"Alt+Tab\" (switch window). "
        "Modifiers: Ctrl, Alt, Shift, Win.\n"
        '  {"action_type": "scroll", "direction": "down", "amount": 3, "description": "..."} — scrolls the '
        "mouse wheel. direction is \"up\"/\"down\"/\"left\"/\"right\". amount is wheel notches (default 3) — "
        "use a bigger amount (e.g. 8-10) to cover more ground in one step instead of repeating scroll actions "
        "many times.\n"
        '  {"action_type": "wait", "wait_ms": 1000, "description": "..."} — does nothing but pause before the '
        "next screenshot. Only use this when the LAST screenshot genuinely looked mid-transition (a page still "
        "loading, a spinner, a window still opening) — not as a default before every step. wait_ms is "
        "300-3000, default 1000.\n"
        '  {"action_type": "open_app", "app_name": "...", "description": "..."} — opens an app via the Start '
        "menu search.\n"
        '  {"action_type": "done", "answer_text": "..."} — the task is complete, or can\'t proceed further; '
        "answer_text summarizes the outcome for the user.\n\n"
        "Clicking is the least reliable action here — small targets (icons, avatars, profile pickers, close "
        "buttons) are easy to miss. Before clicking, check if a keyboard path gets the same result instead "
        "(type a URL/search query and press Enter, or a key_press like Escape/Tab); only click when there's "
        "no keyboard alternative, and if a screen shows something you don't actually need for the task (e.g. "
        "a profile picker on browser launch), try Escape or Tab+Enter before attempting to click its icons.\n"
        "description is a short present-tense phrase shown to the user while this step runs (e.g. \"Clicking "
        "the Jobs tab\"). If \"steps completed so far\" already includes an open_app for the app this task "
        "needs, do NOT call open_app again — assume it opened and look at the screenshot for where to click "
        "or type next, even if the window looks like it's still loading (blank/white is normal right after "
        "launch, not a failure). If the step right before this one was open_app, your next step should almost "
        "always be a click into the middle of that new window's actual content area (not typing yet) — a "
        "freshly opened app doesn't reliably have real keyboard focus on its own editable area even once "
        "it's visually in front, and typing straight into it can silently go nowhere. Only skip this click if "
        "the screenshot already clearly shows a text cursor/caret active in the target field. More generally: "
        "if you've been given the same screen with no visible progress after your last action, don't repeat "
        "that exact action — either try something different or return done with an explanation. "
        "Format your answer as EXACTLY one JSON object, no markdown fences, no extra fields, no commentary."
    )

@router.post("/task", response_model=AgentTaskResponse, dependencies=[Depends(rate_limit)])
async def create_agent_task(request: AgentTaskRequest):
    # Queue the Celery task
    task = run_agent_task.delay(
        request.task_description, request.session_id, request.clipboard_text,
        request.screenshot_base64, request.github_token,
        request.gemini_api_key, request.tavily_api_key,
    )
    return AgentTaskResponse(task_id=task.id)

@router.get("/task/{task_id}", response_model=AgentTaskStatusResponse)
async def get_agent_task_status(task_id: str):
    # Fetch task status from Celery (Redis backend)
    task_result = AsyncResult(task_id)
    
    result_data = None
    if task_result.ready():
        if task_result.successful():
            result_data = task_result.result
        else:
            result_data = str(task_result.info) # Usually exception string
            
    return AgentTaskStatusResponse(
        task_id=task_id,
        status=task_result.status,
        result=result_data
    )

# Sync, not queued through Celery — each step needs to feel responsive as
# the automation runs (the client drives the loop, awaiting one step at a
# time), unlike the normal agent task which is fine polled over ~1-2s.
@router.post("/step", response_model=AgentStepResponse, dependencies=[Depends(rate_limit)])
async def agent_step(request: AgentStepRequest):
    gemini = GeminiService(request.gemini_api_key or settings.gemini_api_key)
    prompt = _build_step_prompt(
        request.task_description, request.plan, request.completed_steps, stuck=request.stuck_on_repeat,
    )

    # "error" (not in _VALID_ACTION_TYPES — the model never emits it, only
    # this route does) is a distinct signal from "done": every fallback below
    # used to return "done" for these, which made a failed Gemini call or a
    # malformed response indistinguishable from genuine task completion on
    # the client (confirmed for real: a Gemini "Server disconnected" error
    # showed up as a green "Completed" run). GeminiService.analyze() itself
    # swallows exceptions into an error STRING rather than raising, so the
    # first except below is defense in depth (base64 decode failure); the
    # second is what actually catches a Gemini-call failure, since that
    # string fails json.loads.
    try:
        image_bytes = base64.b64decode(request.screenshot_base64)
        raw = await gemini.analyze(image_bytes, prompt, json_mode=True)
    except Exception:
        import traceback
        traceback.print_exc()
        return AgentStepResponse(action_type="error", answer_text="Something went wrong deciding the next step.")

    try:
        cleaned = re.sub(r"```(?:json)?\n?", "", raw, flags=re.IGNORECASE).strip()
        parsed = json.loads(cleaned)
    except Exception:
        return AgentStepResponse(action_type="error", answer_text=raw)

    action_type = parsed.get("action_type")
    if action_type not in _VALID_ACTION_TYPES:
        return AgentStepResponse(action_type="error", answer_text="I couldn't figure out what to do next.")

    point = parsed.get("point")
    if not (isinstance(point, list) and len(point) == 2 and all(isinstance(v, (int, float)) for v in point)):
        point = None

    button = parsed.get("button") if parsed.get("button") in ("left", "right") else None
    double = parsed.get("double") if isinstance(parsed.get("double"), bool) else None
    direction = parsed.get("direction") if parsed.get("direction") in ("up", "down", "left", "right") else None
    amount = parsed.get("amount") if isinstance(parsed.get("amount"), (int, float)) else None
    wait_ms = parsed.get("wait_ms") if isinstance(parsed.get("wait_ms"), (int, float)) else None
    if wait_ms is not None:
        wait_ms = max(300, min(3000, int(wait_ms)))  # clamp — a runaway wait shouldn't stall the whole run

    return AgentStepResponse(
        action_type=action_type,
        point=point,
        text=parsed.get("text") if isinstance(parsed.get("text"), str) else None,
        app_name=parsed.get("app_name") if isinstance(parsed.get("app_name"), str) else None,
        key=parsed.get("key") if isinstance(parsed.get("key"), str) else None,
        button=button,
        double=double,
        direction=direction,
        amount=int(amount) if amount is not None else None,
        wait_ms=wait_ms,
        description=parsed.get("description") if isinstance(parsed.get("description"), str) else "",
        answer_text=parsed.get("answer_text") if isinstance(parsed.get("answer_text"), str) else None,
    )
