"""Meeting mode (experimental): suggested answers from a live transcript.

Text only, no screenshot. Stateless on purpose: the transcript and the
user's documents are used for this one request and not written to session
memory or anywhere else.
"""

import base64
import io
import json
import re
import time
import zipfile
from xml.etree import ElementTree

from fastapi import APIRouter, Depends, HTTPException
from fastapi.concurrency import run_in_threadpool
from fastapi.responses import StreamingResponse

from app.models.meeting import CodeSolveRequest, ExtractTextRequest, MeetingAnswerRequest
from app.rate_limit import rate_limit
from app.services.llm import get_llm

router = APIRouter()

# Hard ceiling on transcript text in one prompt, on top of the client's own
# trimming, oldest lines dropped first.
MAX_TRANSCRIPT_CHARS = 12_000

# Hard ceiling on document text in one prompt (~15k tokens): enough for a
# resume plus several project write-ups. Documents are taken in order and
# the one that crosses the line is cut short. The client applies the same
# budget and shows it in Settings.
MAX_DOCUMENTS_CHARS = 60_000


def _format_time(seconds: float) -> str:
    seconds = max(0, int(seconds))
    return f"{seconds // 60:02d}:{seconds % 60:02d}"


def _transcript_block(request: MeetingAnswerRequest) -> str:
    lines = [f"[{_format_time(line.t)}] {line.text.strip()}" for line in request.transcript if line.text.strip()]
    kept: list[str] = []
    total = 0
    for line in reversed(lines):
        total += len(line) + 1
        if kept and total > MAX_TRANSCRIPT_CHARS:
            break
        kept.append(line)
    kept.reverse()
    return "\n".join(kept)


def _documents_block(request: MeetingAnswerRequest) -> str:
    parts: list[str] = []
    budget = MAX_DOCUMENTS_CHARS
    for i, doc in enumerate(request.documents, start=1):
        text = doc.text.strip()
        if not text or budget <= 0:
            continue
        if len(text) > budget:
            text = text[:budget].rstrip() + "\n[…]"
        budget -= len(text)
        name = doc.name.strip() or f"Document {i}"
        parts.append(f'<document name="{name}">\n{text}\n</document>')
    return "\n\n".join(parts)


# Stable across every press in a meeting (instructions, background,
# documents), so it goes first: both providers cache a repeated prompt
# prefix, which cuts time to first token once documents make it long.
# Only the transcript, last, changes between presses.
_INSTRUCTIONS = """You are the user's real-time meeting assistant. When someone in the meeting asks the user something, you write the answer the user will read off their screen and say out loud, in the first person, as themselves.

How to answer:
- About the user (their experience, projects, skills, education, results): the documents and background below are the source of truth. Use their specifics (project names, technologies, numbers, outcomes) whenever they fit the question.
- About anything they don't cover: answer anyway, confidently and correctly, from solid expertise: established best practices, sound engineering judgement, and the logically correct answer. For a personal question the documents don't cover, answer in a way that fits the user's documented background, framed around how they work and what they'd do; don't make up specific employers, dates or figures.
- Never say or imply that information is missing. Never mention documents, a transcript, context, or that you are an AI. Never use placeholders.
- Speech recognition makes mistakes: infer the intended words instead of taking odd phrasings literally.

Format, exactly:
Q: <the question, rephrased in at most 15 words>

<the answer: 1-3 short sentences that lead with the direct answer, natural to say out loud>

- <supporting point, at most 15 words>
- <up to 4 such bullets in total, only if they add something>

For a coding or technical-design question, after the answer sentences add one fenced code block (```language) of at most 25 lines with the key code, then at most 2 bullets (e.g. time/space complexity, the main trade-off).
Plain text otherwise: no headings, no bold, no tables, no preamble, no closing remarks.
If nobody has asked the user anything, write "Q: (no direct question)", then one line on the last point made and a short thing the user could say next."""


def build_prompt(request: MeetingAnswerRequest) -> str:
    brief = request.brief.strip() or "(none)"
    documents = _documents_block(request) or "(none)"
    return f"""{_INSTRUCTIONS}

Background the user wrote:
{brief}

The user's documents:
{documents}

Live transcript of what the OTHER people in the meeting said, oldest first, as minutes:seconds since the user started listening. The user's own speech is not in it.
{_transcript_block(request)}"""


async def _stream_answer(request: MeetingAnswerRequest, llm):
    started = time.monotonic()
    full_text = ""
    async for piece in llm.analyze_stream(None, build_prompt(request)):
        if not piece:
            continue
        full_text += piece
        yield json.dumps({"type": "chunk", "text": piece}) + "\n"
    print(json.dumps({
        "event": "meeting_answer",
        "lines": len(request.transcript),
        "documents": len(request.documents),
        "answer_chars": len(full_text),
        "duration_ms": round((time.monotonic() - started) * 1000, 1),
    }), flush=True)
    yield json.dumps({"type": "done", "answer_text": full_text.strip()}) + "\n"


@router.post("/answer-stream", dependencies=[Depends(rate_limit)])
async def answer_stream(request: MeetingAnswerRequest):
    llm = get_llm(request.provider, request.model, request.gemini_api_key, request.openai_api_key)
    return StreamingResponse(_stream_answer(request, llm), media_type="application/x-ndjson")


# ---------------------------------------------------------------------
# Coding questions shown on screen
# ---------------------------------------------------------------------

_CODE_INSTRUCTIONS = """You are the user's real-time coding-interview assistant. The screenshot shows their screen right now: usually a coding problem, in a shared screen inside a meeting app or on a coding site, possibly with an editor that already has a function signature, a class stub, or the user's own partial code. Solve it so the user can explain the idea and write the code themselves.

How to solve:
- Read the screenshot carefully: the problem statement, examples, constraints, input/output format, and anything already typed in the editor.
- Keep the exact function signature, class and method names from any stub on screen. If the user has partial code, build on it rather than starting over, unless it's wrong.
- Language: if the editor or stub on screen is in a specific language, or the problem names one (a SQL question, "implement in Java"), use that. Otherwise use {language}.
- Give the solution an interviewer expects: correct first, at the optimal or expected time complexity, clean and idiomatic, complete and runnable, no placeholders, only the comments an interviewer would expect. Mention the brute-force idea in one line of the approach when it helps the explanation.
- Constraints said out loud in the transcript below count as part of the problem.
- If part of the problem is cut off or blurry, assume the standard version of that problem and say so in the Problem line. Never say you can't see or don't have enough information.
- If the screen shows no coding problem, use the transcript; if there's none there either, write "### Problem" with "(no coding question found on screen)" and one line of advice.

Format, exactly, with these headings:
Language: <the language used> (<why: "from the editor", "the problem asks for it", or "your preference">)

### Problem
<what is being asked, in one line>

### Approach
<2-4 short lines in the first person that the user can say out loud while starting to code: the idea, the data structure, why it works>

### Code
```<fence tag: python, javascript, typescript, java, cpp, csharp, go, rust, kotlin, swift, sql>
<the complete solution>
```

### Complexity
- Time: <O(...)>, <why in a few words>
- Space: <O(...)>, <why in a few words>

### Edge cases
- <3-5 short bullets: inputs to mention or test>

No other text before or after."""


def build_code_prompt(request: CodeSolveRequest) -> str:
    prompt = _CODE_INSTRUCTIONS.replace("{language}", request.language.strip() or "Python")
    brief = request.brief.strip()
    if brief:
        prompt += f"\n\nBackground the user wrote:\n{brief}"
    lines = [f"[{_format_time(l.t)}] {l.text.strip()}" for l in request.transcript if l.text.strip()]
    prompt += "\n\nRecent transcript of what the other people in the meeting said:\n" + ("\n".join(lines) or "(nothing)")
    previous = request.previous_solution.strip()
    if previous:
        prompt += f"""

The user pressed again, so revise this earlier solution of yours. Account for anything new on screen (a different or changed problem, new code in the editor) or in the transcript (a new constraint, "now optimise it", a follow-up). If nothing changed, fix any mistake and improve it. Give the full updated answer in the same format.
<previous_solution>
{previous}
</previous_solution>"""
    return prompt


async def _stream_code(request: CodeSolveRequest, images: list[bytes], llm):
    started = time.monotonic()
    first_ms = None
    full_text = ""
    async for piece in llm.analyze_images_stream(images, build_code_prompt(request)):
        if not piece:
            continue
        if first_ms is None:
            first_ms = round((time.monotonic() - started) * 1000, 1)
        full_text += piece
        yield json.dumps({"type": "chunk", "text": piece}) + "\n"
    print(json.dumps({
        "event": "meeting_code",
        "model": getattr(llm, "model", ""),
        "screenshots": len(images),
        "revising": bool(request.previous_solution.strip()),
        "first_token_ms": first_ms,
        "duration_ms": round((time.monotonic() - started) * 1000, 1),
    }), flush=True)
    yield json.dumps({"type": "done", "answer_text": full_text.strip()}) + "\n"


@router.post("/code-stream", dependencies=[Depends(rate_limit)])
async def code_stream(request: CodeSolveRequest):
    try:
        images = [base64.b64decode(s, validate=True) for s in request.screenshots]
    except Exception:
        raise HTTPException(status_code=400, detail="Invalid screenshot data.")
    llm = get_llm(request.provider, request.model, request.gemini_api_key, request.openai_api_key)
    return StreamingResponse(_stream_code(request, images, llm), media_type="application/x-ndjson")


# ---------------------------------------------------------------------
# Text extraction for documents the user adds in Settings
# ---------------------------------------------------------------------

_WORD_NS = "{http://schemas.openxmlformats.org/wordprocessingml/2006/main}"


def _tidy(text: str) -> str:
    """Trailing spaces off every line, runs of blank lines down to one."""
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return re.sub(r"\n{3,}", "\n\n", "\n".join(lines)).strip()


def _pdf_text(data: bytes) -> str:
    from pypdf import PdfReader

    reader = PdfReader(io.BytesIO(data))
    if reader.is_encrypted:
        # Many PDFs are "encrypted" with an empty password just to set
        # permissions; those open fine. A real password can't be asked for.
        try:
            if not reader.decrypt(""):
                raise ValueError
        except Exception:
            raise HTTPException(status_code=422, detail="This PDF is password-protected. Remove the password and add it again.")
    return "\n\n".join((page.extract_text() or "") for page in reader.pages)


def _docx_text(data: bytes) -> str:
    """Paragraph text from a .docx (a zip of XML), tables included. No
    extra dependency: the body is word/document.xml, text lives in <w:t>."""
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            root = ElementTree.fromstring(z.read("word/document.xml"))
    except (zipfile.BadZipFile, KeyError, ElementTree.ParseError):
        raise HTTPException(status_code=422, detail="This doesn't look like a valid Word (.docx) file.")
    paragraphs = []
    for p in root.iter(f"{_WORD_NS}p"):
        pieces = []
        for node in p.iter():
            if node.tag == f"{_WORD_NS}t" and node.text:
                pieces.append(node.text)
            elif node.tag == f"{_WORD_NS}tab":
                pieces.append("\t")
            elif node.tag in (f"{_WORD_NS}br", f"{_WORD_NS}cr"):
                pieces.append("\n")
        paragraphs.append("".join(pieces))
    return "\n".join(paragraphs)


def extract_text(filename: str, data: bytes) -> str:
    name = filename.lower()
    if name.endswith(".pdf"):
        text = _pdf_text(data)
    elif name.endswith(".docx"):
        text = _docx_text(data)
    else:
        raise HTTPException(status_code=415, detail="Only PDF and Word (.docx) files are extracted here.")
    text = _tidy(text)
    if not text:
        raise HTTPException(
            status_code=422,
            detail="No text found in this file. If it's a scanned document (pictures of pages), export it with selectable text first.",
        )
    return text


@router.post("/extract-text")
async def extract_text_route(request: ExtractTextRequest):
    """Pulls the text out of a PDF or Word file the user adds as meeting
    context. Nothing is stored: the client keeps the text on its own
    machine. No model call, so not rate-limited like the model routes."""
    try:
        data = base64.b64decode(request.data_base64, validate=True)
    except Exception:
        raise HTTPException(status_code=400, detail="Invalid file data.")
    try:
        text = await run_in_threadpool(extract_text, request.filename, data)
    except HTTPException:
        raise
    except Exception as e:
        raise HTTPException(status_code=422, detail=f"Couldn't read this file ({type(e).__name__}).")
    return {"text": text, "chars": len(text)}
