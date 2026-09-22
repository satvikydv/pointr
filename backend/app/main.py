import json
import time

from fastapi import Depends, FastAPI, Request
from fastapi.middleware.cors import CORSMiddleware
from app.routes import analyze, agent
from app.security import verify_client_key

app = FastAPI(title="Pointr API")

# The model-backed routes — the only ones whose latency is worth watching,
# since everything else here is either instant or a health check.
_TIMED_PATHS = {
    "/api/analyze-screen-stream",
    "/api/agent/task",
    "/api/agent/step",
}


@app.middleware("http")
async def log_request_timing(request: Request, call_next):
    """Structured stdout timing for the model-backed routes.

    Deliberately local-only: one JSON line per request to stdout (picked up
    by `docker compose logs`), no forwarding anywhere. Client-side PostHog
    events already cover product usage, and request bodies here carry
    screenshots, queries and BYOK keys — none of which should ever leave
    the server. Only the path, status and duration are recorded.
    """
    if request.url.path not in _TIMED_PATHS:
        return await call_next(request)

    started = time.perf_counter()
    status = 500
    try:
        response = await call_next(request)
        status = response.status_code
        return response
    finally:
        print(json.dumps({
            "event": "request_timing",
            "path": request.url.path,
            "method": request.method,
            "status": status,
            "duration_ms": round((time.perf_counter() - started) * 1000, 1),
        }), flush=True)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(analyze.router, prefix="/api", dependencies=[Depends(verify_client_key)])
app.include_router(agent.router, prefix="/api/agent", dependencies=[Depends(verify_client_key)])

@app.get("/health")
def health():
    return {"status": "ok"}
