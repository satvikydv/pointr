"""Meeting-mode answer route: prompt contents and the NDJSON stream shape.
The model is faked, and the rate limit (Redis) and client-key checks are
overridden, so this runs without any services:

    cd backend && python -m pytest tests
"""

import json

import pytest
from fastapi.testclient import TestClient

from app import main
from app.models.meeting import MeetingAnswerRequest
from app.rate_limit import rate_limit
from app.routes import meeting


class FakeLLM:
    def __init__(self, pieces):
        self.pieces = pieces
        self.calls = []

    async def analyze_stream(self, image_bytes, prompt):
        self.calls.append((image_bytes, prompt))
        for p in self.pieces:
            yield p


async def _no_limit():
    return None


@pytest.fixture
def client(monkeypatch):
    fake = FakeLLM(["Q: What's your plan?\n\n", "Ship stealth first.", ""])
    seen = {}

    def fake_get_llm(provider, model, gemini_api_key="", openai_api_key=""):
        seen.update(provider=provider, model=model, gemini=gemini_api_key, openai=openai_api_key)
        return fake

    monkeypatch.setattr(meeting, "get_llm", fake_get_llm)
    main.app.dependency_overrides[rate_limit] = _no_limit
    try:
        yield TestClient(main.app), fake, seen
    finally:
        main.app.dependency_overrides.clear()


def _body(**overrides):
    body = {
        "transcript": [
            {"t": 61.2, "text": "Thanks for joining."},
            {"t": 75.0, "text": "So what's your plan for the launch?"},
        ],
        "brief": "I'm the PM for Pointr.",
        "provider": "openai",
        "model": "some-model",
        "openai_api_key": "sk-test",
    }
    body.update(overrides)
    return body


def test_streams_chunks_then_done(client):
    http, fake, seen = client
    res = http.post("/api/meeting/answer-stream", json=_body())
    assert res.status_code == 200
    assert res.headers["content-type"].startswith("application/x-ndjson")

    events = [json.loads(line) for line in res.text.splitlines() if line.strip()]
    assert [e["type"] for e in events] == ["chunk", "chunk", "done"]  # empty piece skipped
    assert events[-1]["answer_text"] == "Q: What's your plan?\n\nShip stealth first."
    assert seen == {"provider": "openai", "model": "some-model", "gemini": "", "openai": "sk-test"}

    image, prompt = fake.calls[0]
    assert image is None, "meeting answers are text-only"
    assert "[01:15] So what's your plan for the launch?" in prompt
    assert "I'm the PM for Pointr." in prompt


def test_rejects_empty_transcript(client):
    http, _, _ = client
    assert http.post("/api/meeting/answer-stream", json=_body(transcript=[])).status_code == 422


def test_prompt_without_brief_and_trims_oldest():
    long_lines = [{"t": float(i), "text": "x" * 1500} for i in range(20)]
    request = MeetingAnswerRequest(transcript=long_lines + [{"t": 999.0, "text": "Any questions?"}])
    prompt = meeting.build_prompt(request)
    assert "(none given)" in prompt
    assert "[16:39] Any questions?" in prompt, "the newest line is always kept"
    assert prompt.count("x" * 1500) < 20, "oldest lines dropped past MAX_TRANSCRIPT_CHARS"
