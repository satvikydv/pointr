from pydantic_settings import BaseSettings, SettingsConfigDict

class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_file=".env", env_file_encoding="utf-8")

    gemini_model: str = "gemini-3.1-flash-lite"
    gemini_api_key: str = ""
    marker_mode: str = "burned_in" # "burned_in" | "coords_only"
    image_max_long_edge: int = 2048
    request_timeout_secs: int = 20
    redis_url: str = "redis://localhost:6379/0"
    # Container-side path the filesystem MCP server is scoped to (see
    # docker-compose.yml's worker volume mount). Empty = not configured, the
    # agent's needs_filesystem path answers with a clear "not set up" message
    # instead of trying to spawn a server against a path that doesn't exist.
    pointr_fs_root: str = ""

settings = Settings()
