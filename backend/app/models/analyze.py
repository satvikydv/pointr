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
    # Either the screenshot inline, or a ref from /api/stage-screenshot
    # uploaded while the user was still typing/speaking (see
    # routes/analyze.resolve_screenshot). Inline wins when both are set.
    screenshot_base64: str = ""
    screenshot_ref: str = ""
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
    # Conversation thread, which (unlike session_id, per foreground app)
    # follows the user across apps for a few minutes, so "now do that in
    # Notepad" can see the question asked in the browser. Empty from older
    # clients: memory then falls back to session_id.
    thread_id: str = Field(default="", max_length=64)
    # What the client knows is happening right now that the server can't
    # (e.g. the walkthrough the user just interrupted with this question).
    extra_context: str = Field(default="", max_length=4000)
    timestamp: datetime
    # BYOK: the user's own Gemini API key, entered in Settings. Empty falls
    # back to the server's own key (settings.gemini_api_key), if any — kept
    # so local dev / a future centralized deployment don't need this set.
    gemini_api_key: str = ""
    # Provider + model chosen in Settings, and the user's OpenAI key when
    # provider is "openai" (BYOK, no server-side fallback). Missing provider
    # means gemini, so older desktop builds keep working unchanged.
    provider: str = "gemini"
    model: str = ""
    openai_api_key: str = ""

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
    # "point" | "box" | "line" | None. point uses only x_norm/y_norm; box and
    # line use x_norm/y_norm as the first corner/endpoint and x2_norm/y2_norm
    # as the second — same two-coordinate-pair shape either way, just
    # rendered differently client-side. Still capped at 2 points per step
    # (matching Gemini's own trained point/box_2d grounding primitives,
    # which top out at 2 corners for a box) rather than opening up arbitrary
    # N-point polygons.
    shape: Optional[str] = None
    x_norm: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    y_norm: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    x2_norm: Optional[float] = Field(default=None, ge=0.0, le=1.0)
    y2_norm: Optional[float] = Field(default=None, ge=0.0, le=1.0)

class StoryboardResponse(BaseModel):
    steps: list[StoryboardStep]
    session_id: str
