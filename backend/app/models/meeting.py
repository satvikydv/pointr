from typing import Annotated

from pydantic import BaseModel, Field


class TranscriptLine(BaseModel):
    # Seconds since the meeting session started, at the end of the phrase.
    t: float = 0.0
    text: str = Field(min_length=1, max_length=2000)


# One uploaded file's text. The client extracts and trims it (see
# /api/meeting/extract-text); this is a backstop.
MAX_DOCUMENT_CHARS = 60_000


class ContextDocument(BaseModel):
    name: str = Field(default="", max_length=200)
    text: str = Field(min_length=1, max_length=MAX_DOCUMENT_CHARS)


class MeetingAnswerRequest(BaseModel):
    """What the desktop client sends on Ctrl+Alt+Enter in meeting mode.

    `transcript` holds what the OTHER participants said (system audio only;
    the user's own mic isn't transcribed), oldest first, already cut down
    client-side to the last few minutes."""

    transcript: list[TranscriptLine] = Field(min_length=1, max_length=400)
    # Free-text background the user wrote in Settings (role, who the
    # meeting is with, notes).
    brief: str = Field(default="", max_length=4000)
    # Documents the user added in Settings (resume, project write-ups,
    # notes): the source of truth about them for answers.
    documents: list[ContextDocument] = Field(default_factory=list, max_length=20)
    # BYOK fields, same meaning as on AnalyzeRequest.
    gemini_api_key: str = ""
    provider: str = "gemini"
    model: str = ""
    openai_api_key: str = ""


# A 15 MB file, base64-encoded.
MAX_UPLOAD_B64 = 20 * 1024 * 1024


class ExtractTextRequest(BaseModel):
    filename: str = Field(min_length=1, max_length=255)
    data_base64: str = Field(min_length=1, max_length=MAX_UPLOAD_B64)


# One screenshot, base64 PNG: a 2400 px crop is a few MB at most.
MAX_SCREENSHOT_B64 = 12 * 1024 * 1024


class CodeSolveRequest(BaseModel):
    """Meeting mode's coding solver (the desktop app's meeting/code.rs): a
    coding question shown on screen, solved with the stronger coding model."""

    screenshots: list[Annotated[str, Field(min_length=1, max_length=MAX_SCREENSHOT_B64)]] = Field(min_length=1, max_length=4)
    # Recent speech, for constraints said out loud; may be empty.
    transcript: list[TranscriptLine] = Field(default_factory=list, max_length=200)
    # Used when neither the screen nor the problem settles the language.
    language: str = Field(default="Python", max_length=30)
    # The last solution in this meeting, when the user pressed again.
    previous_solution: str = Field(default="", max_length=30_000)
    brief: str = Field(default="", max_length=4000)
    gemini_api_key: str = ""
    provider: str = "gemini"
    model: str = ""
    openai_api_key: str = ""
