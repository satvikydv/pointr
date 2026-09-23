"""Picks the model provider for a request.

The desktop app sends provider + model with every request (both chosen in
Settings), plus the user's own key for that provider (BYOK). Everything
downstream takes the returned service and calls the same four methods
regardless of which provider it is.
"""
from app.config import settings
from app.services.gemini import GeminiService
from app.services.openai_service import OpenAIService

PROVIDERS = ("gemini", "openai")

# Keep in sync with DEFAULT_MODELS in the desktop app's settings.rs.
DEFAULT_MODELS = {
    "gemini": "gemini-3.1-flash-lite",
    # gpt-5-mini/nano were the obvious picks but are shut down on
    # 2026-12-11 (per OpenAI's deprecations page); gpt-6-luna is the current
    # catalog's most efficient vision-capable tier.
    "openai": "gpt-6-luna",
}


def normalize_provider(provider: str | None) -> str:
    # Unknown or missing -> gemini, so an older desktop build that sends no
    # provider at all keeps working exactly as before.
    return provider if provider in PROVIDERS else "gemini"


def get_llm(
    provider: str | None,
    model: str | None,
    gemini_api_key: str = "",
    openai_api_key: str = "",
):
    provider = normalize_provider(provider)
    model = (model or "").strip() or DEFAULT_MODELS[provider]
    if provider == "openai":
        # No server-side OpenAI key exists: OpenAI is BYOK only.
        return OpenAIService(openai_api_key, model)
    # Gemini keeps its existing fallback to the server's own .env key.
    return GeminiService(gemini_api_key or settings.gemini_api_key, model)


def api_key_for(provider: str, gemini_api_key: str = "", openai_api_key: str = "") -> str:
    """The key the tool loops should use for this provider."""
    if normalize_provider(provider) == "openai":
        return openai_api_key
    return gemini_api_key or settings.gemini_api_key
