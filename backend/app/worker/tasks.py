import time
from app.worker.celery_app import celery_app

@celery_app.task(bind=True)
def run_agent_task(self, task_description: str, session_id: str):
    """
    A dummy Celery task to simulate a long-running agent.
    """
    # Sleep to simulate long-running work
    time.sleep(5)
    
    # Return a mocked success response
    return {
        "status": "success",
        "result": f"Agent mode is not yet implemented — coming soon. (Received task: '{task_description}' for session {session_id})",
        "pointer_target": None
    }
