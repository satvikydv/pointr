import time
from google import genai
from google.genai import types
from app.config import settings

class GeminiService:
    def __init__(self, api_key: str):
        # Client() raises immediately if api_key is empty (unlike the old
        # google.generativeai SDK, which only failed on first actual call) —
        # keep the same "app starts fine, individual requests get a clear
        # message" behavior by not constructing it at all without a key.
        self.client = genai.Client(api_key=api_key) if api_key else None

    async def analyze(self, image_bytes: bytes, prompt: str) -> str:
        if not self.client:
            return "Gemini API key not configured."

        try:
            image_part = types.Part.from_bytes(data=image_bytes, mime_type="image/png")
            response = await self.client.aio.models.generate_content(
                model=settings.gemini_model,
                contents=[prompt, image_part],
            )
            return response.text
        except Exception as e:
            import traceback
            traceback.print_exc()
            return f"Error communicating with Gemini: {str(e)}"

    def analyze_text_sync(self, prompt: str) -> str:
        """Text-only, blocking call — for the Celery worker, which runs
        tasks in a plain sync context rather than an asyncio event loop."""
        if not self.client:
            return "Gemini API key not configured."

        try:
            response = self.client.models.generate_content(
                model=settings.gemini_model,
                contents=[prompt],
            )
            return response.text
        except Exception as e:
            import traceback
            traceback.print_exc()
            return f"Error communicating with Gemini: {str(e)}"

    async def analyze_stream(self, image_bytes: bytes, prompt: str):
        """Yields the answer as it's generated, instead of waiting for the
        full response. Used by the streaming route so the overlay can show
        real answer text growing live instead of a static loading spinner."""
        if not self.client:
            yield "Gemini API key not configured."
            return

        start = time.monotonic()
        try:
            image_part = types.Part.from_bytes(data=image_bytes, mime_type="image/png")
            print(f"[gemini] calling generate_content_stream, image={len(image_bytes)} bytes, model={settings.gemini_model}")
            response_stream = await self.client.aio.models.generate_content_stream(
                model=settings.gemini_model,
                contents=[prompt, image_part],
            )
            print(f"[gemini] generate_content_stream returned after {time.monotonic() - start:.2f}s")

            first_chunk = True
            async for chunk in response_stream:
                if first_chunk:
                    print(f"[gemini] first chunk after {time.monotonic() - start:.2f}s")
                    first_chunk = False
                if chunk.text:
                    yield chunk.text
            print(f"[gemini] stream fully done after {time.monotonic() - start:.2f}s total")
        except Exception as e:
            import traceback
            traceback.print_exc()
            print(f"[gemini] failed after {time.monotonic() - start:.2f}s: {e}")
            yield f"Error communicating with Gemini: {str(e)}"
