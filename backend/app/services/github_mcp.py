"""GitHub MCP client — connects to GitHub's own remote-hosted MCP server
(https://api.githubcopilot.com/mcp/) over streamable HTTP, no subprocess
needed (unlike the filesystem tool's locally-spawned Node server).

READ_ONLY_TOOLS is a real, live-verified allowlist (via `list_available_tools`
against an actual token), not guessed from naming conventions — GitHub's
server exposes 47 tools total, and guessing wrong in either direction would
either miss a real write tool (e.g. `issue_write`, `pull_request_review_write`
don't read as obviously mutating from name alone if you don't already know
GitHub's naming scheme) or exclude a real read one. The excluded tools:
add_comment_to_pending_review, add_issue_comment, add_reply_to_pull_request_
comment, assign_copilot_to_issue, create_branch, create_or_update_file,
create_pull_request, create_pull_request_with_copilot, create_repository,
delete_file, fork_repository, issue_write, merge_pull_request,
pull_request_review_write, push_files, request_copilot_review,
run_secret_scanning (triggers a scan, not a pure read), sub_issue_write,
update_pull_request, update_pull_request_branch.
"""
import asyncio
from typing import Optional

from google.genai import types
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

GITHUB_MCP_URL = "https://api.githubcopilot.com/mcp/"

READ_ONLY_TOOLS = {
    "get_commit",
    "get_copilot_job_status",
    "get_file_contents",
    "get_label",
    "get_latest_release",
    "get_me",
    "get_release_by_tag",
    "get_tag",
    "get_team_members",
    "get_teams",
    "issue_read",
    "list_branches",
    "list_commits",
    "list_issue_fields",
    "list_issue_types",
    "list_issues",
    "list_pull_requests",
    "list_releases",
    "list_repository_collaborators",
    "list_tags",
    "pull_request_read",
    "search_code",
    "search_commits",
    "search_issues",
    "search_pull_requests",
    "search_repositories",
    "search_users",
}

MAX_TOOL_CALL_STEPS = 8


async def _run(
    prompt: str, model: str, api_key: str, github_token: str, image_bytes: Optional[bytes] = None
) -> str:
    from google import genai
    import httpx2

    client = genai.Client(api_key=api_key)
    http_client = httpx2.AsyncClient(headers={"Authorization": f"Bearer {github_token}"})

    async with streamable_http_client(GITHUB_MCP_URL, http_client=http_client) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools_result = await session.list_tools()
            gh_tools = [t for t in tools_result.tools if t.name in READ_ONLY_TOOLS]

            gemini_tool = types.Tool(function_declarations=[
                types.FunctionDeclaration(
                    name=t.name,
                    description=t.description,
                    parameters_json_schema=t.input_schema,
                )
                for t in gh_tools
            ])
            config = types.GenerateContentConfig(tools=[gemini_tool])
            initial_parts = [types.Part(text=prompt)]
            if image_bytes:
                initial_parts.append(types.Part.from_bytes(data=image_bytes, mime_type="image/png"))
            contents = [types.Content(role="user", parts=initial_parts)]

            for _ in range(MAX_TOOL_CALL_STEPS):
                response = await client.aio.models.generate_content(
                    model=model, contents=contents, config=config,
                )
                fcs = response.function_calls
                if not fcs:
                    return response.text

                contents.append(response.candidates[0].content)
                response_parts = []
                for fc in fcs:
                    result = await session.call_tool(fc.name, fc.args or {})
                    text_out = "\n".join(
                        c.text for c in result.content if hasattr(c, "text")
                    )
                    response_parts.append(
                        types.Part.from_function_response(
                            name=fc.name, response={"result": text_out}
                        )
                    )
                contents.append(types.Content(role="user", parts=response_parts))

            return "I looked through GitHub but couldn't settle on an answer in time — try a more specific request."


def run_agent_turn_with_github_sync(
    prompt: str, model: str, api_key: str, github_token: str, image_bytes: Optional[bytes] = None
) -> str:
    """Blocking entry point for the Celery task (plain sync context, no
    running event loop) — mirrors mcp_filesystem's sync wrapper."""
    return asyncio.run(_run(prompt, model, api_key, github_token, image_bytes))


async def list_available_tools(github_token: str) -> list:
    """Diagnostic only — connects and returns every tool name + description
    the server actually exposes, unfiltered. Used once to build/verify
    READ_ONLY_TOOLS against real data, not guessed names."""
    import httpx2

    http_client = httpx2.AsyncClient(headers={"Authorization": f"Bearer {github_token}"})
    async with streamable_http_client(GITHUB_MCP_URL, http_client=http_client) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools_result = await session.list_tools()
            return [{"name": t.name, "description": t.description} for t in tools_result.tools]
