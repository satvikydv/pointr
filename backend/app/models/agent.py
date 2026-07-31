from pydantic import BaseModel
from typing import Optional, Dict, Any

class AgentTaskRequest(BaseModel):
    task_description: str
    session_id: str

class AgentTaskResponse(BaseModel):
    task_id: str

class AgentTaskStatusResponse(BaseModel):
    task_id: str
    status: str
    result: Optional[Any] = None
