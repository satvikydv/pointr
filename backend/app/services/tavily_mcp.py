"""Tavily MCP client — connects to Tavily's own remote-hosted MCP server
(https://mcp.tavily.com/mcp/) over streamable HTTP, same shape as
services.github_mcp (no subprocess, unlike the filesystem tool).

Tavily's key is passed as a query param on the URL itself (their own
convention, confirmed live), not an Authorization header.

READ_ONLY_TOOLS is a real, live-verified allowlist (via `list_available_tools`
against an actual key) — the server exposes 5 tools total (tavily_search,
tavily_extract, tavily_crawl, tavily_map, tavily_research); all are read-only
by nature (nothing here can mutate anything), but only search + extract are
allowlisted for v1 to keep scope/latency down — crawl/map/research are much
heavier operations not needed for "look this up" style tasks.
"""
import asyncio
from typing import Optional

from google.genai import types
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

TAVILY_MCP_URL_TEMPLATE = "https://mcp.tavily.com/mcp/?tavilyApiKey={key}"

READ_ONLY_TOOLS = {
    "tavily_search",
    "tavily_extract",
}

MAX_TOOL_CALL_STEPS = 6


async def _run(
    prompt: str, model: str, api_key: str, tavily_api_key: str, image_bytes: Optional[bytes] = None
) -> str:
    from google import genai

    client = genai.Client(api_key=api_key)
    url = TAVILY_MCP_URL_TEMPLATE.format(key=tavily_api_key)

    async with streamable_http_client(url) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools_result = await session.list_tools()
            tv_tools = [t for t in tools_result.tools if t.name in READ_ONLY_TOOLS]

            gemini_tool = types.Tool(function_declarations=[
                types.FunctionDeclaration(
                    name=t.name,
                    description=t.description,
                    parameters_json_schema=t.input_schema,
                )
                for t in tv_tools
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

            return "I searched the web but couldn't settle on an answer in time — try a more specific request."


def run_agent_turn_with_web_search_sync(
    prompt: str, model: str, api_key: str, tavily_api_key: str, image_bytes: Optional[bytes] = None
) -> str:
    """Blocking entry point for the Celery task — mirrors github_mcp's sync wrapper."""
    return asyncio.run(_run(prompt, model, api_key, tavily_api_key, image_bytes))


async def list_available_tools(tavily_api_key: str) -> list:
    """Diagnostic only — connects and returns every tool name + description
    the server actually exposes, unfiltered. Used once to build/verify
    READ_ONLY_TOOLS against real data, not guessed names."""
    url = TAVILY_MCP_URL_TEMPLATE.format(key=tavily_api_key)
    async with streamable_http_client(url) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools_result = await session.list_tools()
            return [{"name": t.name, "description": t.description} for t in tools_result.tools]
