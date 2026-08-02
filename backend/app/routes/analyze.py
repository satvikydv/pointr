import base64
import json
from fastapi import APIRouter, Depends, HTTPException
from fastapi.responses import StreamingResponse
from app.models.analyze import AnalyzeRequest, AnalyzeResponse, PointerTarget
from app.dependencies import get_gemini_service
from app.services.gemini import GeminiService

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

    prompt = (
        f"The user's cursor is at normalized position x={request.cursor_position.x_norm:.2f}, "
        f"y={request.cursor_position.y_norm:.2f}. "
        f"The active application is '{request.active_window_title}'. "
        f"The user asked: '{request.query_text}'. "
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
    return (
        f"The user's cursor is at normalized position x={request.cursor_position.x_norm:.2f}, "
        f"y={request.cursor_position.y_norm:.2f}. "
        f"The active application is '{request.active_window_title}'. "
        f"The user asked: '{request.query_text}'. "
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
