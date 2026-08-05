import base64
import json
import re
from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import StreamingResponse
from app.models.analyze import AnalyzeRequest, AnalyzeResponse, PointerTarget, StoryboardResponse, StoryboardStep
from app.dependencies import get_gemini_service
from app.services.gemini import GeminiService
from app.services.session_memory import build_session_context_block, record_exchange

router = APIRouter()

# Sentinel line the model appends (per prompt instructions in _build_stream_prompt)
# when it wants to point at a specific UI element. Kept off-screen during
# streaming so the user only ever sees clean prose, never a half-typed marker.
POINTER_SENTINEL = "POINTER:"

@router.post("/analyze-screen", response_model=AnalyzeResponse)
async def analyze_screen(
    request: AnalyzeRequest,
    gemini: GeminiService = Depends(get_gemini_service)
):
    try:
        # Decode image
        image_bytes = base64.b64decode(request.screenshot_base64)
    except Exception as e:
        raise HTTPException(status_code=400, detail="Invalid base64 image")

    session_context = build_session_context_block(request)

    prompt = (
        f"The user's cursor is at normalized position x={request.cursor_position.x_norm:.2f}, "
        f"y={request.cursor_position.y_norm:.2f}. "
        f"Active window (reported by the OS, not a guess — trust it over anything you infer from the image): '{request.active_window_title}'. "
        + (f"{session_context}\n" if session_context else "")
        + f"The user asked: '{request.query_text}'. "
        "Answer their question concisely, referring to what's near the cursor if relevant. "
        "If a specific UI element should be pointed at in your answer, return its approximate bounding box center in the same normalized coordinate space. "
        "Format your answer EXACTLY as JSON:\n"
        "{\n"
        '  "answer_text": "your text here",\n'
        '  // pointer_target is optional. x_norm and y_norm must be floats between 0.0 and 1.0 relative to the top-left of the provided image.\n'
        '  "pointer_target": {"x_norm": 0.5, "y_norm": 0.5, "confidence": "high"}\n'
        "}"
    )

    try:
        result_text = await gemini.analyze(image_bytes, prompt)
        
        # Parse JSON from result_text
        import json
        import re
        
        # Clean up possible markdown formatting
        cleaned_text = re.sub(r'```(?:json)?\n?', '', result_text, flags=re.IGNORECASE).strip()
        parsed = json.loads(cleaned_text)
        
        answer_text = parsed.get("answer_text", "Sorry, I couldn't understand the request.")
        pointer = parsed.get("pointer_target")
        
        pointer_target = None
        if pointer:
            pointer_target = PointerTarget(
                x_norm=pointer.get("x_norm", 0.0),
                y_norm=pointer.get("y_norm", 0.0),
                confidence=pointer.get("confidence", "medium")
            )

        record_exchange(request.session_id, request.query_text, answer_text)

        return AnalyzeResponse(
            answer_text=answer_text,
            pointer_target=pointer_target,
            session_id=request.session_id
        )

    except Exception as e:
        # Fallback if parsing fails
        import traceback
        traceback.print_exc()
        return AnalyzeResponse(
            answer_text=f"Raw response: {result_text if 'result_text' in locals() else str(e)}",
            pointer_target=None,
            session_id=request.session_id
        )


def _build_stream_prompt(request: AnalyzeRequest) -> str:
    # Plain prose instead of the strict-JSON contract used by /analyze-screen:
    # streaming partial JSON would show the user broken, half-typed syntax.
    # The optional pointer is instead a trailing line the client strips out
    # before displaying anything (see stream_analyze below).
    session_context = build_session_context_block(request)

    return (
        f"The user's cursor is at normalized position x={request.cursor_position.x_norm:.2f}, "
        f"y={request.cursor_position.y_norm:.2f}. "
        f"Active window (reported by the OS, not a guess — trust it over anything you infer from the image): '{request.active_window_title}'. "
        + (f"{session_context}\n" if session_context else "")
        + f"The user asked: '{request.query_text}'. "
        "Answer their question concisely in plain text, referring to what's near the cursor if relevant. "
        "Do not use markdown or JSON in your answer.\n"
        "If — and only if — a specific UI element should be pointed at, end your response with exactly "
        "one extra line formatted as:\n"
        f'{POINTER_SENTINEL} {{"x_norm": 0.5, "y_norm": 0.5, "confidence": "high"}}\n'
        "x_norm and y_norm must be floats between 0.0 and 1.0 relative to the top-left of the image. "
        "Omit that line entirely if there's nothing specific to point at."
    )


def _clamp_unit(value) -> float:
    # Gemini is asked for 0.0-1.0 but isn't a hard contract — it occasionally
    # returns something else entirely (e.g. a pixel-space number like 690).
    # Left unclamped, that lands the tooltip thousands of px off-screen.
    try:
        value = float(value)
    except (TypeError, ValueError):
        return 0.0
    return max(0.0, min(1.0, value))


async def _stream_analyze_events(request: AnalyzeRequest, gemini: GeminiService):
    prompt = _build_stream_prompt(request)
    image_bytes = base64.b64decode(request.screenshot_base64)

    full_text = ""
    emitted_len = 0

    async for piece in gemini.analyze_stream(image_bytes, prompt):
        full_text += piece

        sentinel_pos = full_text.find(POINTER_SENTINEL)
        if sentinel_pos != -1:
            safe_end = sentinel_pos
        else:
            # Hold back a trailing fragment that could still turn into the
            # sentinel on the next chunk, so it never flashes on screen.
            hold_back = 0
            max_check = min(len(POINTER_SENTINEL), len(full_text))
            for k in range(max_check, 0, -1):
                if full_text.endswith(POINTER_SENTINEL[:k]):
                    hold_back = k
                    break
            safe_end = len(full_text) - hold_back

        if safe_end > emitted_len:
            yield json.dumps({"type": "chunk", "text": full_text[emitted_len:safe_end]}) + "\n"
            emitted_len = safe_end

    pointer_target = None
    sentinel_pos = full_text.find(POINTER_SENTINEL)
    if sentinel_pos != -1:
        answer_text = full_text[:sentinel_pos].rstrip()
        pointer_line = full_text[sentinel_pos + len(POINTER_SENTINEL):].strip()
        try:
            pointer_json = json.loads(pointer_line)
            pointer_target = {
                "x_norm": _clamp_unit(pointer_json.get("x_norm", 0.0)),
                "y_norm": _clamp_unit(pointer_json.get("y_norm", 0.0)),
                "confidence": pointer_json.get("confidence", "medium"),
            }
        except Exception:
            pointer_target = None
    else:
        answer_text = full_text.strip()

    # Flush anything still held back (e.g. the whole answer had no pointer
    # line, so the trailing hold-back from the last chunk was never emitted).
    if emitted_len < len(answer_text):
        remaining = answer_text[emitted_len:]
        if remaining:
            yield json.dumps({"type": "chunk", "text": remaining}) + "\n"

    record_exchange(request.session_id, request.query_text, answer_text)

    yield json.dumps({
        "type": "done",
        "answer_text": answer_text,
        "pointer_target": pointer_target,
    }) + "\n"


@router.post("/analyze-screen-stream")
async def analyze_screen_stream(
    request: AnalyzeRequest,
    gemini: GeminiService = Depends(get_gemini_service)
):
    try:
        base64.b64decode(request.screenshot_base64)
    except Exception:
        raise HTTPException(status_code=400, detail="Invalid base64 image")

    return StreamingResponse(
        _stream_analyze_events(request, gemini),
        media_type="application/x-ndjson",
    )


@router.post("/analyze-explain", response_model=StoryboardResponse)
async def analyze_explain(
    request: AnalyzeRequest,
    gemini: GeminiService = Depends(get_gemini_service)
):
    """'explain: <topic>' mode — a short multi-step walkthrough (narration +
    optional point-at-marker per step) instead of one answer, played back
    sequentially by the client with TTS between steps. Non-streaming: the
    client needs the whole step list up front to play it back in order."""
    try:
        image_bytes = base64.b64decode(request.screenshot_base64)
    except Exception:
        raise HTTPException(status_code=400, detail="Invalid base64 image")

    session_context = build_session_context_block(request)

    prompt = (
        f"Active window (reported by the OS, trust it over anything you infer from the image): '{request.active_window_title}'. "
        + (f"{session_context}\n" if session_context else "")
        + f"The user wants a short step-by-step visual walkthrough explaining: '{request.query_text}'. "
        "First, actually look at the screenshot: is there a diagram, figure, chart, piece of UI, or document "
        "on screen that relates to this topic, even loosely? If there is ANY relevant visual on screen, you "
        "MUST use it — walk through it directly (e.g. \"here's the hypotenuse\", \"this is the middle element\") "
        "and point at the specific part you're talking about in most steps, rather than giving a generic "
        "textbook explanation that ignores what's actually shown. Only explain purely abstractly, with no "
        "coordinates at all, if the screen truly has nothing relevant to this topic.\n"
        "Break the explanation into 3 to 6 short steps, each one sentence suitable for being spoken aloud. "
        "For each step, add ONE visual annotation if it helps — pick whichever fits best:\n"
        '  - "point": [y, x] — a single spot, e.g. a specific label or element\n'
        '  - "box_2d": [ymin, xmin, ymax, xmax] — a region/area, e.g. a whole shape, a group, a UI panel\n'
        '  - "line": [[y1, x1], [y2, x2]] — connects/indicates a relationship between two spots, e.g. a side '
        "of a triangle, an edge, drawn as an arrow from the first point to the second\n"
        "All coordinates normalized to 0-1000, y before x, using your normal grounding format. Omit all three "
        "for a step that's genuinely about a general concept with nothing on screen to annotate.\n"
        "Respond with ONLY this JSON, no markdown fences, no extra commentary:\n"
        "{\n"
        '  "steps": [\n'
        '    {"narration": "one short spoken sentence", "point": [300, 400]},\n'
        '    {"narration": "another step", "box_2d": [200, 150, 500, 600]},\n'
        '    {"narration": "another step", "line": [[300, 200], [450, 500]]},\n'
        '    {"narration": "a step with nothing to point at"}\n'
        "  ]\n"
        "}"
    )

    try:
        result_text = await gemini.analyze(image_bytes, prompt)
        cleaned = re.sub(r'```(?:json)?\n?', '', result_text, flags=re.IGNORECASE).strip()
        parsed = json.loads(cleaned)

        steps = []
        for raw_step in parsed.get("steps", []):
            narration = raw_step.get("narration", "").strip()
            if not narration:
                continue

            # Gemini's actual trained grounding formats are [y, x] point and
            # [ymin, xmin, ymax, xmax] box_2d, both on a 0-1000 scale (see
            # Google's "Spatial understanding" docs) — not the x-first/
            # 0.0-1.0 scheme originally guessed for point, which is almost
            # certainly why every marker landed in the same corner regardless
            # of image content (asking for a format the model wasn't trained
            # to produce, rather than one it actually knows). "line" isn't a
            # trained primitive at all — it's just two points reusing the
            # same point mechanism, so expect it to be somewhat less
            # reliable than point/box.
            point = raw_step.get("point")
            box = raw_step.get("box_2d")
            line = raw_step.get("line")

            shape = None
            x_norm = y_norm = x2_norm = y2_norm = None

            if isinstance(box, list) and len(box) == 4:
                shape = "box"
                ymin, xmin, ymax, xmax = box
                x_norm, y_norm = _clamp_unit(xmin / 1000), _clamp_unit(ymin / 1000)
                x2_norm, y2_norm = _clamp_unit(xmax / 1000), _clamp_unit(ymax / 1000)
            elif isinstance(line, list) and len(line) == 2:
                shape = "line"
                (y1, x1), (y2, x2) = line
                x_norm, y_norm = _clamp_unit(x1 / 1000), _clamp_unit(y1 / 1000)
                x2_norm, y2_norm = _clamp_unit(x2 / 1000), _clamp_unit(y2 / 1000)
            elif isinstance(point, list) and len(point) == 2:
                shape = "point"
                x_norm, y_norm = _clamp_unit(point[1] / 1000), _clamp_unit(point[0] / 1000)

            steps.append(StoryboardStep(
                narration=narration,
                shape=shape,
                x_norm=x_norm,
                y_norm=y_norm,
                x2_norm=x2_norm,
                y2_norm=y2_norm,
            ))

        if not steps:
            steps = [StoryboardStep(narration="Sorry, I couldn't put together an explanation for that.")]

        record_exchange(request.session_id, f"explain: {request.query_text}", " ".join(s.narration for s in steps))

        return StoryboardResponse(steps=steps, session_id=request.session_id)

    except Exception:
        import traceback
        traceback.print_exc()
        return StoryboardResponse(
            steps=[StoryboardStep(narration="Sorry, something went wrong putting that explanation together.")],
            session_id=request.session_id,
        )
