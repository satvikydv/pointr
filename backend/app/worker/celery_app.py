import os
from celery import Celery

REDIS_URL = os.getenv("REDIS_URL", "redis://redis:6379/0")

celery_app = Celery(
    "pointr_worker",
    broker=REDIS_URL,
    backend=REDIS_URL,
    include=["app.worker.tasks"]
)

celery_app.conf.update(
    task_serializer="json",
    accept_content=["json"],
    result_serializer="json",
    timezone="UTC",
    enable_utc=True,
    # Celery's default is 24h. A task result holds the answer and plan text,
    # and the client polls for it for at most ~60s after submitting, so
    # 5 minutes leaves generous headroom while keeping retention honest.
    result_expires=300,
)
