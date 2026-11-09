"""Meeting-mode answer route: prompt contents and the NDJSON stream shape.
The model is faked, and the rate limit (Redis) and client-key checks are
overridden, so this runs without any services:

    cd backend && python -m pytest tests
"""

import base64
import io
import json
import zipfile

import pytest
from fastapi.testclient import TestClient

from app import main
from app.models.meeting import MeetingAnswerRequest
from app.rate_limit import rate_limit
from app.security import verify_client_key
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
    main.app.dependency_overrides[verify_client_key] = _no_limit
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
    assert "Background the user wrote:\n(none)" in prompt
    assert "The user's documents:\n(none)" in prompt
    assert "[16:39] Any questions?" in prompt, "the newest line is always kept"
    assert prompt.count("x" * 1500) < 20, "oldest lines dropped past MAX_TRANSCRIPT_CHARS"


# ---------------------------------------------------------------------
# Documents as context
# ---------------------------------------------------------------------


def test_documents_go_in_the_prompt_before_the_transcript():
    request = MeetingAnswerRequest(
        transcript=[{"t": 5.0, "text": "Tell me about your last project."}],
        documents=[
            {"name": "resume.pdf", "text": "Built Pointr, a Tauri screen assistant."},
            {"name": "", "text": "Led the move from REST polling to SSE."},
        ],
    )
    prompt = meeting.build_prompt(request)
    assert '<document name="resume.pdf">\nBuilt Pointr, a Tauri screen assistant.\n</document>' in prompt
    assert '<document name="Document 2">' in prompt
    # Stable parts first, transcript last, so providers can cache the prefix.
    assert prompt.index("<document") < prompt.index("[00:05] Tell me about your last project.")
    assert prompt.rstrip().endswith("[00:05] Tell me about your last project.")


def test_prompt_never_asks_for_missing_context():
    prompt = meeting.build_prompt(MeetingAnswerRequest(transcript=[{"t": 1.0, "text": "Why Rust?"}]))
    assert "Never say or imply that information is missing" in prompt
    assert "placeholder" not in prompt.replace("Never use placeholders", "")


def test_documents_are_cut_to_the_budget():
    big = "a" * (meeting.MAX_DOCUMENTS_CHARS - 100)
    request = MeetingAnswerRequest(
        transcript=[{"t": 1.0, "text": "Hi"}],
        documents=[{"name": "one", "text": big}, {"name": "two", "text": "b" * 5000}, {"name": "three", "text": "c" * 10}],
    )
    block = meeting._documents_block(request)
    assert block.count("a") >= len(big)
    assert '<document name="two">' in block and "[…]" in block, "the one crossing the budget is cut short"
    assert '<document name="three">' not in block, "nothing after the budget is spent"


def _docx(paragraphs):
    ns = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
    body = "".join(f'<w:p><w:r><w:t xml:space="preserve">{p}</w:t></w:r></w:p>' for p in paragraphs)
    xml = f'<?xml version="1.0"?><w:document xmlns:w="{ns}"><w:body>{body}</w:body></w:document>'
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("word/document.xml", xml)
    return buf.getvalue()


def _pdf(text):
    """A minimal one-page PDF showing `text` in Helvetica."""
    stream = f"BT /F1 12 Tf 72 720 Td ({text}) Tj ET".encode()
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = io.BytesIO()
    out.write(b"%PDF-1.4\n")
    offsets = []
    for i, body in enumerate(objects, start=1):
        offsets.append(out.tell())
        out.write(f"{i} 0 obj\n".encode() + body + b"\nendobj\n")
    xref = out.tell()
    out.write(f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode())
    for off in offsets:
        out.write(f"{off:010d} 00000 n \n".encode())
    out.write(f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    return out.getvalue()


def _extract(http, filename, data):
    return http.post(
        "/api/meeting/extract-text",
        json={"filename": filename, "data_base64": base64.b64encode(data).decode()},
    )


def test_extracts_docx(client):
    http, _, _ = client
    res = _extract(http, "Resume.DOCX", _docx(["Satvik Yadav", "", "", "", "Built Pointr"]))
    assert res.status_code == 200
    assert res.json()["text"] == "Satvik Yadav\n\nBuilt Pointr", "blank-line runs collapse to one"


def test_extracts_pdf(client):
    http, _, _ = client
    res = _extract(http, "resume.pdf", _pdf("Senior engineer, Rust and Python"))
    assert res.status_code == 200, res.text
    assert "Senior engineer, Rust and Python" in res.json()["text"]


def test_extract_rejects_unsupported_empty_and_garbage(client):
    http, _, _ = client
    assert _extract(http, "photo.png", b"\x89PNG").status_code == 415
    assert _extract(http, "empty.docx", _docx([])).status_code == 422
    assert _extract(http, "broken.docx", b"not a zip").status_code == 422
    bad = http.post("/api/meeting/extract-text", json={"filename": "a.pdf", "data_base64": "%%%"})
    assert bad.status_code == 400


# ---------------------------------------------------------------------
# Coding questions shown on screen
# ---------------------------------------------------------------------


class FakeVisionLLM:
    model = "fake-coding-model"

    def __init__(self, pieces):
        self.pieces = pieces
        self.calls = []

    async def analyze_images_stream(self, images, prompt):
        self.calls.append((images, prompt))
        for p in self.pieces:
            yield p


@pytest.fixture
def code_client(monkeypatch):
    fake = FakeVisionLLM(["Language: Java (from the editor)\n\n### Problem\nTwo sum", "\n\n### Code\n```java\nclass S {}\n```"])
    seen = {}

    def fake_get_llm(provider, model, gemini_api_key="", openai_api_key=""):
        seen.update(provider=provider, model=model)
        return fake

    monkeypatch.setattr(meeting, "get_llm", fake_get_llm)
    main.app.dependency_overrides[rate_limit] = _no_limit
    main.app.dependency_overrides[verify_client_key] = _no_limit
    try:
        yield TestClient(main.app), fake, seen
    finally:
        main.app.dependency_overrides.clear()


def _code_body(**overrides):
    body = {
        "screenshots": [base64.b64encode(b"\x89PNG fake").decode()],
        "transcript": [{"t": 30.0, "text": "Assume the array is sorted."}],
        "language": "Go",
        "provider": "openai",
        "model": "gpt-6-sol",
    }
    body.update(overrides)
    return body


def test_code_stream_sends_images_and_streams(code_client):
    http, fake, seen = code_client
    res = http.post("/api/meeting/code-stream", json=_code_body())
    assert res.status_code == 200
    events = [json.loads(line) for line in res.text.splitlines() if line.strip()]
    assert [e["type"] for e in events] == ["chunk", "chunk", "done"]
    assert events[-1]["answer_text"].startswith("Language: Java (from the editor)")
    assert seen == {"provider": "openai", "model": "gpt-6-sol"}, "the coding model from the request is used"

    images, prompt = fake.calls[0]
    assert images == [b"\x89PNG fake"]
    assert "Otherwise use Go." in prompt
    assert "[00:30] Assume the array is sorted." in prompt
    assert "previous_solution" not in prompt, "a first press isn't a revision"
    assert "### Edge cases" in prompt


def test_code_stream_revises_previous_solution(code_client):
    http, fake, _ = code_client
    res = http.post("/api/meeting/code-stream", json=_code_body(previous_solution="### Code\nold answer", transcript=[]))
    assert res.status_code == 200
    _, prompt = fake.calls[0]
    assert "<previous_solution>\n### Code\nold answer\n</previous_solution>" in prompt
    assert "(nothing)" in prompt, "empty transcript is stated, not omitted"


def test_code_stream_rejects_bad_input(code_client):
    http, _, _ = code_client
    assert http.post("/api/meeting/code-stream", json=_code_body(screenshots=[])).status_code == 422
    assert http.post("/api/meeting/code-stream", json=_code_body(screenshots=["%%%"])).status_code == 400
    assert http.post("/api/meeting/code-stream", json=_code_body(screenshots=["a"] * 5)).status_code == 422
