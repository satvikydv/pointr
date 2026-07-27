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
