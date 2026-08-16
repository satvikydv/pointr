from fastapi import Depends, FastAPI
from fastapi.middleware.cors import CORSMiddleware
from app.routes import analyze, agent
from app.security import verify_client_key

app = FastAPI(title="Pointr API")

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
