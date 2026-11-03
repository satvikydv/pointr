from pydantic import BaseModel, Field


class TranscriptLine(BaseModel):
    # Seconds since the meeting session started, at the end of the phrase.
    t: float = 0.0
    text: str = Field(min_length=1, max_length=2000)


class MeetingAnswerRequest(BaseModel):
    """What the desktop client sends on Ctrl+Alt+Enter in meeting mode.

    `transcript` holds what the OTHER participants said (system audio only;
    the user's own mic isn't transcribed), oldest first, already cut down
    client-side to the last few minutes."""

    transcript: list[TranscriptLine] = Field(min_length=1, max_length=400)
    # Free-text background the user wrote in Settings (role, who the
    # meeting is with, notes).
    brief: str = Field(default="", max_length=4000)
    # BYOK fields, same meaning as on AnalyzeRequest.
    gemini_api_key: str = ""
    provider: str = "gemini"
    model: str = ""
    openai_api_key: str = ""
