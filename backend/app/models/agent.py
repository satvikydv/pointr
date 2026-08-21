from pydantic import BaseModel
from typing import Optional, Dict, Any, List

class AgentTaskRequest(BaseModel):
    task_description: str
    session_id: str
    clipboard_text: str = ""
    screenshot_base64: str = ""
    github_token: str = ""
    # BYOK — see AnalyzeRequest.gemini_api_key for the fallback rule.
    gemini_api_key: str = ""
    tavily_api_key: str = ""

class AgentTaskResponse(BaseModel):
    task_id: str

class AgentTaskStatusResponse(BaseModel):
    task_id: str
    status: str
    result: Optional[Any] = None

class AgentStepRequest(BaseModel):
    task_description: str
    plan: List[str] = []
    completed_steps: List[str] = []
    screenshot_base64: str
    gemini_api_key: str = ""
    # Set by the client when it detected the model about to propose the
    # exact same action (by real parameters, not the free-text description)
    # it just proposed last call — a targeted, forceful correction for this
    # one call, since the general "don't repeat" instruction in the prompt
    # isn't reliably followed on its own.
    stuck_on_repeat: bool = False

class AgentStepResponse(BaseModel):
    # "error" is backend-synthesized only (Gemini call failed, malformed
    # response) — the model itself never emits it, distinct from "done" so
    # a failed step is never mistaken for a completed one.
    action_type: str  # "click" | "type_text" | "open_app" | "key_press" | "scroll" | "wait" | "done" | "error"
    point: Optional[List[float]] = None  # [y, x], 0-1000 — Gemini's native grounding format
    text: Optional[str] = None
    app_name: Optional[str] = None
    key: Optional[str] = None  # bare ("Enter") or a "+"-joined combo ("Ctrl+S")
    button: Optional[str] = None  # "left" | "right", for click
    double: Optional[bool] = None  # double-click, for click
    direction: Optional[str] = None  # "up" | "down" | "left" | "right", for scroll
    amount: Optional[int] = None  # wheel notches, for scroll
    wait_ms: Optional[int] = None  # for wait
    description: str = ""
    answer_text: Optional[str] = None
