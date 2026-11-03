"""Meeting mode (experimental): suggested answers from a live transcript.

Text only, no screenshot. Stateless on purpose: the transcript is used for
this one request and not written to session memory or anywhere else, since
it holds other people's words.
"""

import json
import time

from fastapi import APIRouter, Depends
from fastapi.responses import StreamingResponse

from app.models.meeting import MeetingAnswerRequest
from app.rate_limit import rate_limit
from app.services.llm import get_llm

router = APIRouter()

# Hard ceiling on transcript text in one prompt, on top of the client's own
# trimming, oldest lines dropped first.
MAX_TRANSCRIPT_CHARS = 12_000


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


def build_prompt(request: MeetingAnswerRequest) -> str:
    brief = request.brief.strip() or "(none given)"
    return f"""You are helping the user in a live meeting, in real time. They will read your reply off their screen and say it out loud, so it has to be quick to read and natural to speak.

Below is an automatic transcript of what the OTHER people in the meeting said, oldest first, with minutes:seconds since the user started listening. The user's own speech is NOT in it. Speech recognition makes mistakes: infer the intended words from context instead of taking odd phrasings literally.

Background from the user:
{brief}

Transcript:
{_transcript_block(request)}

Find the most recent question or request that's aimed at the user and write what the user could say in reply, in the first person.

Format exactly:
Q: <that question, rephrased in at most 15 words>

<the answer: 1-3 short spoken sentences that lead with the direct answer>
- <up to 3 short bullet points with supporting specifics, only if they add something>

Rules: no preamble, no "Great question", no headings, no markdown other than the "- " bullets. Use the background above when it's relevant and don't invent facts about the user beyond it; where a personal detail is needed and unknown, write a short [placeholder]. If there's no question in the transcript, write "Q: (no direct question)" and then one line on the last point made and a short thing the user could say next."""


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
        "answer_chars": len(full_text),
        "duration_ms": round((time.monotonic() - started) * 1000, 1),
    }), flush=True)
    yield json.dumps({"type": "done", "answer_text": full_text.strip()}) + "\n"


@router.post("/answer-stream", dependencies=[Depends(rate_limit)])
async def answer_stream(request: MeetingAnswerRequest):
    llm = get_llm(request.provider, request.model, request.gemini_api_key, request.openai_api_key)
    return StreamingResponse(_stream_answer(request, llm), media_type="application/x-ndjson")
