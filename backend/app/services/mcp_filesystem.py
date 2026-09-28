"""Filesystem MCP client — the agent tool's first real MCP connection.

Read-only for v1: only list/read/search tools are ever declared to Gemini,
never write_file/edit_file/create_directory/move_file, even though the
spawned server exposes those too. The docker-compose volume mount is also
read-only (`:ro`) as a second, independent guardrail — the tool filter here
and the mount's read-only flag both have to be bypassed for a write to ever
reach a real file.

Spawns `npx @modelcontextprotocol/server-filesystem <root>` as a subprocess
over stdio per call rather than keeping a server running — simpler lifecycle,
and this is only invoked on the (rare) agent task that actually asked for
filesystem access, not on every agent turn, so the ~1-2s subprocess startup
cost isn't paid by the common case.
"""
import asyncio
import os
from typing import Any

from app.services.tool_loop import ToolSpec, mcp_result_text, mcp_tool_specs, run_tool_loop
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

# The server exposes several mutating tools (write_file, edit_file,
# create_directory, move_file) — deliberately never declared to the model,
# so there's nothing for it to even attempt to call.
READ_ONLY_TOOLS = {
    "list_directory",
    "list_directory_with_sizes",
    "directory_tree",
    "read_text_file",
    "read_multiple_files",
    "search_files",
    "get_file_info",
    "list_allowed_directories",
}

# read_text_file (the MCP server's own tool) reads PDFs as raw bytes decoded
# as text, which is garbage — PDF is a binary format, not text. This is a
# local tool (not provided by the MCP server) declared alongside the MCP
# ones and executed directly here via pypdf, not through session.call_tool.
PDF_TOOL_NAME = "read_pdf_text"
PDF_TOOL = ToolSpec(
    name=PDF_TOOL_NAME,
    description=(
        "Extract and return the text content of a PDF file. Use this instead of "
        "read_text_file whenever the path ends in .pdf."
    ),
    parameters={
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Absolute path to the .pdf file, within the allowed folder."},
        },
        "required": ["path"],
    },
)
MAX_PDF_CHARS = 12000

MAX_TOOL_CALL_STEPS = 8


def _extract_pdf_text(path: str, fs_root: str) -> str:
    from pypdf import PdfReader
    from pypdf.errors import PdfReadError

    # Defense in depth alongside the read-only bind mount: refuse anything
    # that resolves outside fs_root, even though the container has nothing
    # sensitive mounted anywhere else.
    resolved = os.path.realpath(path)
    root = os.path.realpath(fs_root)
    if os.path.commonpath([resolved, root]) != root:
        return f"Error: '{path}' is outside the allowed folder."
    if not resolved.lower().endswith(".pdf"):
        return f"Error: '{path}' isn't a .pdf file."
    if not os.path.isfile(resolved):
        return f"Error: '{path}' doesn't exist."

    try:
        reader = PdfReader(resolved)
    except PdfReadError as e:
        return f"Error: couldn't open '{path}' as a PDF ({e})."
    except Exception as e:
        return f"Error reading '{path}': {e}"

    if reader.is_encrypted:
        return f"Error: '{path}' is password-protected — can't extract text."

    text = "\n".join((page.extract_text() or "") for page in reader.pages)
    text = text.strip()
    if not text:
        return f"'{path}' has no extractable text (likely a scanned/image-only PDF)."
    if len(text) > MAX_PDF_CHARS:
        text = text[:MAX_PDF_CHARS] + "\n...(truncated)"
    return text


async def _run(
    prompt: str, model: str, api_key: str, fs_root: str,
    image_bytes: bytes | None = None, provider: str = "gemini",
) -> str:
    params = StdioServerParameters(
        command="npx",
        args=["-y", "@modelcontextprotocol/server-filesystem", fs_root],
    )

    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools_result = await session.list_tools()
            mcp_tools = [t for t in tools_result.tools if t.name in READ_ONLY_TOOLS]

            async def execute(name: str, args: dict) -> str:
                if name == PDF_TOOL_NAME:
                    # Local tool, not an MCP one — runs in-process via pypdf
                    # rather than session.call_tool. Blocking, so off the
                    # event loop.
                    return await asyncio.to_thread(_extract_pdf_text, args.get("path", ""), fs_root)
                return mcp_result_text(await session.call_tool(name, args))

            # Screen context matters here too — e.g. "read my resume and
            # answer this question on the form on screen" needs both the file
            # tools AND to see what the form is actually asking, in the same
            # call, or the model can only do one half of the task.
            return await run_tool_loop(
                provider=provider, api_key=api_key, model=model, prompt=prompt,
                image_bytes=image_bytes, tools=mcp_tool_specs(mcp_tools) + [PDF_TOOL],
                execute=execute, max_steps=MAX_TOOL_CALL_STEPS,
                exhausted_message="I looked through the files but couldn't settle on an answer in time — try a more specific request.",
            )


def run_agent_turn_with_filesystem_sync(
    prompt: str, model: str, api_key: str, fs_root: str,
    image_bytes: bytes | None = None, provider: str = "gemini",
) -> str:
    """Blocking entry point for the Celery task (plain sync context, no
    running event loop) — mirrors GeminiService's *_sync methods."""
    return asyncio.run(_run(prompt, model, api_key, fs_root, image_bytes, provider))
