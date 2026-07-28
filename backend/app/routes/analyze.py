import base64
from fastapi import APIRouter, Depends, HTTPException
from app.models.analyze import AnalyzeRequest, AnalyzeResponse, PointerTarget
from app.dependencies import get_gemini_service
from app.services.gemini import GeminiService

router = APIRouter()

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
        '  "pointer_target": {"x_norm": 0.5, "y_norm": 0.5, "confidence": "high"} // optional\n'
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
