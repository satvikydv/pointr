import base64
import json
import re
from fastapi import APIRouter
from celery.result import AsyncResult
from app.models.agent import (
    AgentTaskRequest, AgentTaskResponse, AgentTaskStatusResponse,
    AgentStepRequest, AgentStepResponse,
)
from app.worker.tasks import run_agent_task
from app.services.gemini import GeminiService
from app.config import settings

router = APIRouter()

_VALID_ACTION_TYPES = {"click", "type_text", "open_app", "key_press", "done"}


def _build_step_prompt(task_description: str, plan: list, completed_steps: list) -> str:
    plan_block = "\n".join(f"{i+1}. {s}" for i, s in enumerate(plan)) if plan else "(no plan given)"
    completed_block = "\n".join(f"- {s}" for s in completed_steps) if completed_steps else "(none yet)"
    return (
        "You are executing a multi-step desktop automation task, ONE action at a time. You'll be called "
        "again after each action with a fresh screenshot, so don't try to plan ahead — just decide the "
        "single best next action given what's actually on screen right now.\n\n"
        f"Overall task: {task_description}\n\n"
        f"Rough plan (a guide, not a script — deviate from it if the screen shows something different):\n"
        f"{plan_block}\n\n"
        f"Steps completed so far:\n{completed_block}\n\n"
        "Look at the attached screenshot and decide ONE next action, formatted as one of:\n"
        '  {"action_type": "click", "point": [y, x], "description": "..."} — click a screen location. '
        "point is [y, x] normalized 0-1000 (Gemini's native grounding format — NOT 0-1 fractions, NOT pixels, "
        "y BEFORE x). Aim for the center of the actual clickable element.\n"
        '  {"action_type": "type_text", "text": "...", "description": "..."} — types into whatever is '
        "currently focused. Click the target field first (as a separate step) if nothing is focused yet.\n"
        '  {"action_type": "key_press", "key": "Enter", "description": "..."} — presses one key. Supported: '
        "Enter, Tab, Escape, Backspace, ArrowDown, ArrowUp, ArrowLeft, ArrowRight.\n"
        '  {"action_type": "open_app", "app_name": "...", "description": "..."} — opens an app via the Start '
        "menu search.\n"
        '  {"action_type": "done", "answer_text": "..."} — the task is complete, or can\'t proceed further; '
        "answer_text summarizes the outcome for the user.\n\n"
        "description is a short present-tense phrase shown to the user while this step runs (e.g. \"Clicking "
        "the Jobs tab\"). If \"steps completed so far\" already includes an open_app for the app this task "
        "needs, do NOT call open_app again — assume it opened and look at the screenshot for where to click "
        "or type next, even if the window looks like it's still loading (blank/white is normal right after "
        "launch, not a failure). More generally: if you've been given the same screen with no visible "
        "progress after your last action, don't repeat that exact action — either try something different "
        "or return done with an explanation. "
        "Format your answer as EXACTLY one JSON object, no markdown fences, no extra fields, no commentary."
    )

@router.post("/task", response_model=AgentTaskResponse)
async def create_agent_task(request: AgentTaskRequest):
    # Queue the Celery task
    task = run_agent_task.delay(request.task_description, request.session_id, request.clipboard_text, request.screenshot_base64)
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
@router.post("/step", response_model=AgentStepResponse)
async def agent_step(request: AgentStepRequest):
    gemini = GeminiService(settings.gemini_api_key)
    prompt = _build_step_prompt(request.task_description, request.plan, request.completed_steps)

    try:
        image_bytes = base64.b64decode(request.screenshot_base64)
        raw = await gemini.analyze(image_bytes, prompt)
    except Exception:
        import traceback
        traceback.print_exc()
        return AgentStepResponse(action_type="done", answer_text="Something went wrong deciding the next step.")

    try:
        cleaned = re.sub(r"```(?:json)?\n?", "", raw, flags=re.IGNORECASE).strip()
        parsed = json.loads(cleaned)
    except Exception:
        return AgentStepResponse(action_type="done", answer_text=raw)

    action_type = parsed.get("action_type")
    if action_type not in _VALID_ACTION_TYPES:
        return AgentStepResponse(action_type="done", answer_text="I couldn't figure out what to do next.")

    point = parsed.get("point")
    if not (isinstance(point, list) and len(point) == 2 and all(isinstance(v, (int, float)) for v in point)):
        point = None

    return AgentStepResponse(
        action_type=action_type,
        point=point,
        text=parsed.get("text") if isinstance(parsed.get("text"), str) else None,
        app_name=parsed.get("app_name") if isinstance(parsed.get("app_name"), str) else None,
        key=parsed.get("key") if isinstance(parsed.get("key"), str) else None,
        description=parsed.get("description") if isinstance(parsed.get("description"), str) else "",
        answer_text=parsed.get("answer_text") if isinstance(parsed.get("answer_text"), str) else None,
    )
