import google.generativeai as genai
from app.config import settings

class GeminiService:
    def __init__(self, api_key: str):
        if api_key:
            genai.configure(api_key=api_key)
        self.model = genai.GenerativeModel(settings.gemini_model)

    async def analyze(self, image_bytes: bytes, prompt: str) -> str:
        if not settings.gemini_api_key:
            return "Gemini API key not configured."
            
        try:
            # We assume image_bytes is a valid image (e.g., PNG)
            image_parts = [
                {
                    "mime_type": "image/png",
                    "data": image_bytes
                }
            ]
            
            response = await self.model.generate_content_async([prompt, image_parts[0]])
            return response.text
        except Exception as e:
            import traceback
            traceback.print_exc()
            return f"Error communicating with Gemini: {str(e)}"
