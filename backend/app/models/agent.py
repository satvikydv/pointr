from pydantic import BaseModel
from typing import Optional, Dict, Any, List

class AgentTaskRequest(BaseModel):
    task_description: str
    session_id: str
    clipboard_text: str = ""
    screenshot_base64: str = ""
    github_token: str = ""

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

class AgentStepResponse(BaseModel):
    action_type: str  # "click" | "type_text" | "open_app" | "key_press" | "done"
    point: Optional[List[float]] = None  # [y, x], 0-1000 — Gemini's native grounding format
    text: Optional[str] = None
    app_name: Optional[str] = None
    key: Optional[str] = None
    description: str = ""
    answer_text: Optional[str] = None
