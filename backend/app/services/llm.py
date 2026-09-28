"""Picks the model provider for a request.

The desktop app sends provider + model with every request (both chosen in
Settings), plus the user's own key for that provider (BYOK). Everything
downstream takes the returned service and calls the same four methods
regardless of which provider it is.
"""
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
    # BYOK only for both providers — no server-side key backs either one.
    # Gemini used to fall back to settings.gemini_api_key; dropped
    # 2026-09-28 so cost/quota is never carried on the operator's own key.
    if provider == "openai":
        return OpenAIService(openai_api_key, model)
    return GeminiService(gemini_api_key, model)


def api_key_for(provider: str, gemini_api_key: str = "", openai_api_key: str = "") -> str:
    """The key the tool loops should use for this provider."""
    if normalize_provider(provider) == "openai":
        return openai_api_key
    return gemini_api_key
