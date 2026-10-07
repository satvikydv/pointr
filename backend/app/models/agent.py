from pydantic import BaseModel
from typing import Optional, Dict, Any, List

class AgentTaskRequest(BaseModel):
    task_description: str
    session_id: str
    # See AnalyzeRequest.thread_id.
    thread_id: str = ""
    clipboard_text: str = ""
    screenshot_base64: str = ""
    # Staged upload ref, same as AnalyzeRequest.screenshot_ref.
    screenshot_ref: str = ""
    github_token: str = ""
    # BYOK — see AnalyzeRequest.gemini_api_key for the fallback rule.
    gemini_api_key: str = ""
    # Provider + model chosen in Settings, and the user's OpenAI key when
    # provider is "openai" (BYOK, no server-side fallback). Missing provider
    # means gemini, so older desktop builds keep working unchanged.
    provider: str = "gemini"
    model: str = ""
    openai_api_key: str = ""
    tavily_api_key: str = ""

class AgentTaskResponse(BaseModel):
    task_id: str

class AgentTaskStatusResponse(BaseModel):
    task_id: str
    status: str
    result: Optional[Any] = None

class AgentStepRequest(BaseModel):
    task_description: str
    # Lets a finished on-screen task be remembered (see session_memory), so
    # the next question or task can refer back to what just happened.
    session_id: str = ""
    thread_id: str = ""
    plan: List[str] = []
    completed_steps: List[str] = []
    screenshot_base64: str
    gemini_api_key: str = ""
    # Provider + model chosen in Settings, and the user's OpenAI key when
    # provider is "openai" (BYOK, no server-side fallback). Missing provider
    # means gemini, so older desktop builds keep working unchanged.
    provider: str = "gemini"
    model: str = ""
    openai_api_key: str = ""
    # Set by the client when it detected the model about to propose the
    # exact same action (by real parameters, not the free-text description)
    # it just proposed last call — a targeted, forceful correction for this
    # one call, since the general "don't repeat" instruction in the prompt
    # isn't reliably followed on its own.
    stuck_on_repeat: bool = False
    # JSON string from the client's browser_snapshot command (Rust ->
    # local Playwright driver) — present only when a browser_open has
    # already run this task. Supplements screenshot_base64, doesn't
    # replace it: the model still benefits from seeing the page visually,
    # but picks click/type targets by accessibility ref from this instead
    # of guessing a pixel coordinate.
    browser_snapshot: Optional[str] = None

class AgentStepResponse(BaseModel):
    # "error" is backend-synthesized only (Gemini call failed, malformed
    # response) — the model itself never emits it, distinct from "done" so
    # a failed step is never mistaken for a completed one.
    action_type: str  # "click" | "type_text" | "open_app" | "key_press" | "scroll" | "wait" | "browser_open" | "browser_navigate" | "browser_click" | "browser_type" | "browser_close" | "done" | "error"
    point: Optional[List[float]] = None  # [y, x], 0-1000 — Gemini's native grounding format
    text: Optional[str] = None
    app_name: Optional[str] = None
    key: Optional[str] = None  # bare ("Enter") or a "+"-joined combo ("Ctrl+S")
    button: Optional[str] = None  # "left" | "right", for click
    double: Optional[bool] = None  # double-click, for click
    direction: Optional[str] = None  # "up" | "down" | "left" | "right", for scroll
    amount: Optional[int] = None  # wheel notches, for scroll
    wait_ms: Optional[int] = None  # for wait
    url: Optional[str] = None  # for browser_navigate
    ref: Optional[str] = None  # accessibility ref from the last browser_snapshot, for browser_click/browser_type
    description: str = ""
    answer_text: Optional[str] = None
