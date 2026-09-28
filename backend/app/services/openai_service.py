"""OpenAI provider — same method surface as GeminiService (analyze,
analyze_stream, analyze_sync, analyze_text_sync) so every route and task
can take either one from services.llm.get_llm() without branching.

Uses the Responses API (openai SDK 3.x). Request/response shapes were
confirmed against the installed SDK's own type definitions, not written
from memory: input_text/input_image content parts, text.format json_object,
the "response.output_text.delta" stream event, and Response.output_text.

store=False on every call. The Responses API otherwise keeps each
response on OpenAI's side for 30 days so it can be referenced later — this
app never references one, and retaining users' screenshots and questions
at a third party for a month would contradict the privacy page.
"""
import base64

from openai import APIStatusError, AsyncOpenAI, OpenAI

# Error strings deliberately mirror GeminiService's ("Error communicating
# with <provider>: <status> ...") so the client's disguised-error detection
# and the upstream_status telemetry property work for both providers.
_NO_KEY = "OpenAI API key not configured."


def _error_text(e: Exception) -> str:
    if isinstance(e, APIStatusError):
        return f"Error communicating with OpenAI: {e.status_code} {e.message}"
    return f"Error communicating with OpenAI: {e}"


def _user_input(prompt: str, image_bytes: bytes | None) -> list:
    content = [{"type": "input_text", "text": prompt}]
    if image_bytes:
        b64 = base64.b64encode(image_bytes).decode("ascii")
        content.append({
            "type": "input_image",
            "image_url": f"data:image/png;base64,{b64}",
            # Screenshots are full desktops with small UI text; "high" keeps
            # that legible instead of letting the model downscale it away.
            "detail": "high",
        })
    return [{"role": "user", "content": content}]


class OpenAIService:
    provider = "openai"

    def __init__(self, api_key: str, model: str):
        # Same "no key -> clear per-request message" behavior as GeminiService.
        self.async_client = AsyncOpenAI(api_key=api_key) if api_key else None
        self.sync_client = OpenAI(api_key=api_key) if api_key else None
        self.model = model

    async def analyze(self, image_bytes: bytes, prompt: str, json_mode: bool = False) -> str:
        if not self.async_client:
            return _NO_KEY
        try:
            kwargs = {"text": {"format": {"type": "json_object"}}} if json_mode else {}
            response = await self.async_client.responses.create(
                model=self.model, input=_user_input(prompt, image_bytes), store=False, **kwargs,
            )
            return response.output_text
        except Exception as e:
            import traceback
            traceback.print_exc()
            return _error_text(e)

    def analyze_text_sync(self, prompt: str, use_search: bool = False) -> str:
        # use_search is a Gemini-only concept (Google Search grounding) and
        # currently has no callers; accepted here only to keep the surface
        # identical. Live web search goes through the Tavily tool instead.
        return self.analyze_sync(None, prompt)

    def analyze_sync(self, image_bytes: bytes | None, prompt: str, use_search: bool = False) -> str:
        if not self.sync_client:
            return _NO_KEY
        try:
            response = self.sync_client.responses.create(
                model=self.model, input=_user_input(prompt, image_bytes), store=False,
            )
            return response.output_text
        except Exception as e:
            import traceback
            traceback.print_exc()
            return _error_text(e)

    async def analyze_stream(self, image_bytes: bytes, prompt: str):
        if not self.async_client:
            yield _NO_KEY
            return
        try:
            stream = await self.async_client.responses.create(
                model=self.model, input=_user_input(prompt, image_bytes), store=False, stream=True,
            )
            async for event in stream:
                if event.type == "response.output_text.delta" and event.delta:
                    yield event.delta
        except Exception as e:
            import traceback
            traceback.print_exc()
            yield _error_text(e)
