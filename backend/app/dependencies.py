from functools import lru_cache
from app.services.gemini import GeminiService
from app.config import settings

@lru_cache
def get_gemini_service() -> GeminiService:
    return GeminiService(api_key=settings.gemini_api_key)
