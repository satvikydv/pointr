from pydantic import BaseModel, Field
from datetime import datetime
from typing import Optional

class CursorPosition(BaseModel):
    x_norm: float = Field(ge=0.0, le=1.0)
    y_norm: float = Field(ge=0.0, le=1.0)

class ScreenResolution(BaseModel):
    width: int
    height: int

class AnalyzeRequest(BaseModel):
    screenshot_base64: str
    cursor_position: CursorPosition
    screen_resolution: ScreenResolution
    active_window_title: str
    # Separate from active_window_title (which also carries filename/language)
    # so the backend can key session continuity off the app alone, without
    # parsing a display string back apart.
    app_name: str = ""
    # How long the user has stayed in app_name without switching, as tracked
    # client-side (see Rust's SessionState) — 0 on the first capture in a
    # new session.
    session_duration_secs: float = 0.0
    query_text: str = Field(min_length=1, max_length=2000)
    session_id: str
    timestamp: datetime

class PointerTarget(BaseModel):
    x_norm: float = Field(ge=0.0, le=1.0)
    y_norm: float = Field(ge=0.0, le=1.0)
    confidence: str   # "low" | "medium" | "high"

class AnalyzeResponse(BaseModel):
    answer_text: str
    pointer_target: Optional[PointerTarget] = None
    session_id: str

class StoryboardStep(BaseModel):
    narration: str
    # Deliberately just a single point per step (reusing the same marker the
    # normal flow already renders), not arrows/boxes/regions — asking the
    # model for one coordinate pair is already the failure class that's bitten
    # this project twice (see pointer_target clamping); a multi-shape-per-step
    # contract would multiply that risk for a first version.
    x_norm: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    y_norm: Optional[float] = Field(default=None, ge=0.0, le=1.0)

class StoryboardResponse(BaseModel):
    steps: list[StoryboardStep]
    session_id: str
