from fastapi import APIRouter
from celery.result import AsyncResult
from app.models.agent import AgentTaskRequest, AgentTaskResponse, AgentTaskStatusResponse
from app.worker.tasks import run_agent_task

router = APIRouter()

@router.post("/task", response_model=AgentTaskResponse)
async def create_agent_task(request: AgentTaskRequest):
    # Queue the Celery task
    task = run_agent_task.delay(request.task_description, request.session_id, request.clipboard_text, request.screenshot_base64)
    return AgentTaskResponse(task_id=task.id)

@router.get("/task/{task_id}", response_model=AgentTaskStatusResponse)
async def get_agent_task_status(task_id: str):
    # Fetch task status from Celery (Redis backend)
    task_result = AsyncResult(task_id)
    
    result_data = None
    if task_result.ready():
        if task_result.successful():
            result_data = task_result.result
        else:
            result_data = str(task_result.info) # Usually exception string
            
    return AgentTaskStatusResponse(
        task_id=task_id,
        status=task_result.status,
        result=result_data
    )
