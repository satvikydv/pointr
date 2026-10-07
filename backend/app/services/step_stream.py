"""Pulls complete JSON objects out of a model's streamed text, as they finish.

Used by the streaming explain route: the model is asked for one JSON object
per line (one per step), and each step is sent on to the client the moment it
is complete, so the first sentence can be spoken while the rest is still
being written. Deliberately tolerant about what surrounds the objects: models
add markdown fences, commas, a surrounding array or a {"steps": [...]}
wrapper no matter what the prompt says, and none of that should lose a step.
"""

import json


class JsonObjectStream:
    def __init__(self) -> None:
        self._depth = 0
        self._in_string = False
        self._escaped = False
        self._chunk: list[str] = []

    def feed(self, text: str) -> list[dict]:
        """Returns the objects completed by `text`, in order. Anything outside
        an object (fences, commas, brackets, prose) is skipped; an object that
        completes but is not valid JSON is dropped rather than raised."""
        done: list[dict] = []
        for ch in text:
            if self._depth == 0:
                if ch == "{":
                    self._depth = 1
                    self._chunk = ["{"]
                continue

            self._chunk.append(ch)
            if self._in_string:
                if self._escaped:
                    self._escaped = False
                elif ch == "\\":
                    self._escaped = True
                elif ch == '"':
                    self._in_string = False
            elif ch == '"':
                self._in_string = True
            elif ch == "{":
                self._depth += 1
            elif ch == "}":
                self._depth -= 1
                if self._depth == 0:
                    raw = "".join(self._chunk)
                    self._chunk = []
                    try:
                        obj = json.loads(raw)
                    except ValueError:
                        continue
                    if isinstance(obj, dict):
                        done.append(obj)
        return done


def expand_steps(obj: dict) -> list[dict]:
    """A completed object is normally one step, but a model that ignores the
    one-per-line instruction may wrap them all as {"steps": [...]}."""
    steps = obj.get("steps")
    if isinstance(steps, list):
        return [s for s in steps if isinstance(s, dict)]
    return [obj]
