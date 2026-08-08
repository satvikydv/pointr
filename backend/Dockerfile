# Stage 1: build deps
FROM python:3.12-slim AS builder
WORKDIR /app
COPY requirements.txt .
RUN pip install --user --no-cache-dir -r requirements.txt

# Stage 2: runtime
FROM python:3.12-slim AS runtime
WORKDIR /app
# Node/npx: only the worker needs this, to run the filesystem MCP server
# (`npx @modelcontextprotocol/server-filesystem`) as a subprocess — installed
# in the shared image rather than maintaining a second Dockerfile just for
# the worker's one extra dependency.
RUN apt-get update && apt-get install -y --no-install-recommends nodejs npm && \
    apt-get clean && rm -rf /var/lib/apt/lists/*
COPY --from=builder /root/.local /root/.local
COPY app/ ./app/
ENV PATH=/root/.local/bin:$PATH
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
