import time
from fastapi import HTTPException, Request
from redis.asyncio import Redis
from app.config import settings

# Lazy — module import shouldn't require Redis to be reachable yet (e.g.
# during tests or before docker-compose brings redis up).
_redis: Redis | None = None


def _get_redis() -> Redis:
    global _redis
    if _redis is None:
        _redis = Redis.from_url(settings.redis_url, decode_responses=True)
    return _redis


def _client_ip(request: Request) -> str:
    # Behind Caddy, request.client.host is the proxy's own docker-network
    # IP for every request — the real client IP is in X-Forwarded-For
    # (which Caddy's reverse_proxy sets by default).
    xff = request.headers.get("x-forwarded-for")
    if xff:
        return xff.split(",")[0].strip()
    return request.client.host if request.client else "unknown"


async def rate_limit(request: Request):
    """Fixed one-minute window per IP, applied only to the specific routes
    that actually call Gemini/Tavily (see main.py-adjacent route decorators)
    — the cheap GET status-poll endpoint deliberately isn't gated by this,
    since a single legitimate agent task polls it dozens of times/minute on
    its own."""
    ip = _client_ip(request)
    window = int(time.time() // 60)
    key = f"ratelimit:{ip}:{window}"
    r = _get_redis()
    count = await r.incr(key)
    if count == 1:
        await r.expire(key, 65)
    if count > settings.rate_limit_per_minute:
        raise HTTPException(status_code=429, detail="Too many requests — slow down and try again in a moment.")
