"""One tool-calling loop, shared by the GitHub, Tavily and filesystem tools.

Each of those services used to carry its own copy of the same Gemini
function-calling loop, differing only in how it connected and which tools
it allowed. Adding a second provider would have meant six copies, so the
loop lives here once, with one branch per provider. The services now only
handle what's genuinely theirs: connecting to their MCP server, choosing
which tools to expose, and executing a call.

The Gemini branch is the previous per-service loop unchanged. The OpenAI
branch uses the Responses API statelessly (store=False) — prior output
items, including encrypted reasoning, are passed back as input on each
turn instead of being referenced by id from OpenAI-side storage.
"""
import base64
import json
from dataclasses import dataclass
from typing import Awaitable, Callable

from app.services.llm import normalize_provider


@dataclass
class ToolSpec:
    name: str
    description: str
    parameters: dict


# (tool name, arguments) -> result text. Callers route MCP tools to their
# session and local tools (e.g. the filesystem's PDF reader) in-process.
Executor = Callable[[str, dict], Awaitable[str]]


async def run_tool_loop(
    *,
    provider: str,
    api_key: str,
    model: str,
    prompt: str,
    image_bytes: bytes | None,
    tools: list[ToolSpec],
    execute: Executor,
    max_steps: int,
    exhausted_message: str,
) -> str:
    if normalize_provider(provider) == "openai":
        return await _openai_loop(api_key, model, prompt, image_bytes, tools, execute, max_steps, exhausted_message)
    return await _gemini_loop(api_key, model, prompt, image_bytes, tools, execute, max_steps, exhausted_message)


async def _gemini_loop(api_key, model, prompt, image_bytes, tools, execute, max_steps, exhausted_message) -> str:
    from google import genai
    from google.genai import types

    client = genai.Client(api_key=api_key)
    gemini_tool = types.Tool(function_declarations=[
        types.FunctionDeclaration(name=t.name, description=t.description, parameters_json_schema=t.parameters)
        for t in tools
    ])
    config = types.GenerateContentConfig(tools=[gemini_tool])

    parts = [types.Part(text=prompt)]
    if image_bytes:
        parts.append(types.Part.from_bytes(data=image_bytes, mime_type="image/png"))
    contents = [types.Content(role="user", parts=parts)]

    for _ in range(max_steps):
        response = await client.aio.models.generate_content(model=model, contents=contents, config=config)
        fcs = response.function_calls
        if not fcs:
            return response.text

        contents.append(response.candidates[0].content)
        results = []
        for fc in fcs:
            text_out = await execute(fc.name, fc.args or {})
            results.append(types.Part.from_function_response(name=fc.name, response={"result": text_out}))
        contents.append(types.Content(role="user", parts=results))

    # Plain text (not JSON) on purpose, so the caller's fallback parsing
    # kicks in rather than silently returning nothing.
    return exhausted_message


def _openai_tool_defs(tools: list[ToolSpec]) -> list[dict]:
    return [
        {
            "type": "function",
            "name": t.name,
            "description": t.description or "",
            "parameters": t.parameters or {"type": "object", "properties": {}},
            # Required by the SDK, with no default. Strict mode demands every
            # property be required and additionalProperties false, which
            # third-party MCP schemas don't guarantee — so it's off.
            "strict": False,
        }
        for t in tools
    ]


async def _openai_loop(api_key, model, prompt, image_bytes, tools, execute, max_steps, exhausted_message) -> str:
    from openai import AsyncOpenAI

    client = AsyncOpenAI(api_key=api_key)
    content = [{"type": "input_text", "text": prompt}]
    if image_bytes:
        b64 = base64.b64encode(image_bytes).decode("ascii")
        content.append({"type": "input_image", "image_url": f"data:image/png;base64,{b64}", "detail": "high"})
    items: list = [{"role": "user", "content": content}]
    tool_defs = _openai_tool_defs(tools)

    for _ in range(max_steps):
        response = await client.responses.create(
            model=model,
            input=items,
            tools=tool_defs,
            store=False,
            # Reasoning models need their reasoning passed back on the next
            # turn; with store=False it has to travel encrypted in the
            # request rather than be looked up server-side.
            include=["reasoning.encrypted_content"],
        )
        calls = [o for o in response.output if o.type == "function_call"]
        if not calls:
            return response.output_text

        items.extend(o.model_dump(mode="json", exclude_none=True, by_alias=True) for o in response.output)
        for call in calls:
            try:
                args = json.loads(call.arguments or "{}")
            except json.JSONDecodeError:
                args = {}
            text_out = await execute(call.name, args)
            items.append({"type": "function_call_output", "call_id": call.call_id, "output": text_out})

    return exhausted_message


def mcp_tool_specs(mcp_tools) -> list[ToolSpec]:
    return [ToolSpec(name=t.name, description=t.description or "", parameters=t.input_schema) for t in mcp_tools]


def mcp_result_text(result) -> str:
    return "\n".join(c.text for c in result.content if hasattr(c, "text"))
