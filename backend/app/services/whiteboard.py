"""Whiteboard shapes for "explain" walkthroughs.

When nothing on screen relates to the topic, the model teaches on a blank
board instead: each step carries a short list of shapes to draw while its
sentence is spoken, and the diagram builds up step by step. This module is
the contract for those shapes. Whatever the model writes is reduced to the
few shapes the client knows how to draw, with every number clamped to the
canvas and every string trimmed, so a sloppy or oversized reply can't break
the drawing or flood the screen.

Canvas: 1000 wide by 600 tall, x from the left, y from the top. Shapes refer
to each other by short ids (an arrow from "client" to "server"); the ids are
checked by the client, which knows every shape drawn so far.
"""

CANVAS_W = 1000
CANVAS_H = 600
MAX_SHAPES_PER_STEP = 8
COLORS = {"white", "blue", "red", "green", "yellow"}
SIZES = {"s", "m", "l"}

PROMPT = (
    "WHITEBOARD: if nothing on screen relates to the topic, do not leave the steps bare. Teach it on a "
    "blank whiteboard instead, drawing a simple diagram that builds up as you speak, like a teacher at a "
    "board. Give each step a \"board\" list of the shapes to draw while that sentence is spoken (1 to 4 "
    "per step). Later steps add to the same board and point back at earlier shapes; never redraw a shape. "
    "The canvas is 1000 wide and 600 tall, x from the left edge, y from the top. Shapes:\n"
    '  {"type": "box", "id": "db", "x": 80, "y": 60, "w": 220, "h": 90, "label": "Database", "color": "blue"} '
    "(x, y is the top-left corner)\n"
    '  {"type": "circle", "id": "sun", "x": 500, "y": 300, "r": 70, "label": "Sun", "color": "yellow"} '
    "(x, y is the centre)\n"
    '  {"type": "text", "x": 500, "y": 560, "text": "E = mc²", "size": "l", "color": "white"} '
    "(x, y is the centre of the text; size s, m or l)\n"
    '  {"type": "arrow", "from": "db", "to": "sun", "label": "reads", "color": "white"} '
    '(from and to are ids of shapes already drawn, or a spot like {"x": 120, "y": 400})\n'
    '  {"type": "highlight", "target": "db"} (circles a shape that is already drawn, to point back at it)\n'
    "Colors: white, blue, red, green, yellow. Labels are a few words, not sentences. Give every box and "
    "circle an id. Spread the diagram over the whole canvas, keep everything inside it, and never let "
    "shapes overlap; leave at least 150 units between shapes joined by an arrow so the arrow and its "
    "label have room. Step 1 draws at most 2 shapes so it can start right away. A step that only adds "
    "a highlight or an arrow is fine. "
    "Never mix board shapes with point, box_2d or line in the same walkthrough.\n"
)

PROMPT_FORCED = (
    "The user asked for this on the whiteboard: ignore the screen and draw every step on the board.\n"
)


def _num(value, lo, hi, default=None):
    try:
        n = float(value)
    except (TypeError, ValueError):
        return default
    if n != n:  # NaN
        return default
    return round(min(hi, max(lo, n)), 1)


def _text(value, limit):
    if value is None:
        return ""
    return " ".join(str(value).split())[:limit]


def _id(value):
    text = _text(value, 24)
    return text or None


def _color(value):
    color = _text(value, 12).lower()
    return color if color in COLORS else "white"


def _endpoint(value):
    """An arrow end: a shape id, or a spot on the canvas."""
    if isinstance(value, dict):
        x = _num(value.get("x"), 0, CANVAS_W)
        y = _num(value.get("y"), 0, CANVAS_H)
        return {"x": x, "y": y} if x is not None and y is not None else None
    if isinstance(value, (list, tuple)) and len(value) == 2:
        x = _num(value[0], 0, CANVAS_W)
        y = _num(value[1], 0, CANVAS_H)
        return {"x": x, "y": y} if x is not None and y is not None else None
    return _id(value)


def _shape(raw):
    if not isinstance(raw, dict):
        return None
    kind = _text(raw.get("type"), 12).lower()
    color = _color(raw.get("color"))

    if kind in ("box", "rect", "rectangle"):
        x = _num(raw.get("x"), 0, CANVAS_W - 20)
        y = _num(raw.get("y"), 0, CANVAS_H - 20)
        w = _num(raw.get("w", raw.get("width")), 20, CANVAS_W, 160)
        h = _num(raw.get("h", raw.get("height")), 20, CANVAS_H, 80)
        if x is None or y is None:
            return None
        # Keep the whole box on the canvas.
        w = min(w, CANVAS_W - x)
        h = min(h, CANVAS_H - y)
        return {"type": "box", "id": _id(raw.get("id")), "x": x, "y": y, "w": w, "h": h,
                "label": _text(raw.get("label"), 40), "color": color}

    if kind in ("circle", "ellipse"):
        x = _num(raw.get("x"), 0, CANVAS_W)
        y = _num(raw.get("y"), 0, CANVAS_H)
        r = _num(raw.get("r", raw.get("radius")), 10, 280, 60)
        if x is None or y is None:
            return None
        r = min(r, x, CANVAS_W - x, y, CANVAS_H - y) or 10
        return {"type": "circle", "id": _id(raw.get("id")), "x": x, "y": y, "r": max(10, r),
                "label": _text(raw.get("label"), 40), "color": color}

    if kind in ("text", "label"):
        x = _num(raw.get("x"), 0, CANVAS_W)
        y = _num(raw.get("y"), 0, CANVAS_H)
        text = _text(raw.get("text", raw.get("label")), 80)
        if x is None or y is None or not text:
            return None
        size = _text(raw.get("size"), 2).lower()
        return {"type": "text", "id": _id(raw.get("id")), "x": x, "y": y, "text": text,
                "size": size if size in SIZES else "m", "color": color}

    if kind in ("arrow", "line", "connector"):
        start = _endpoint(raw.get("from"))
        end = _endpoint(raw.get("to"))
        if start is None or end is None or start == end:
            return None
        return {"type": "arrow", "from": start, "to": end,
                "label": _text(raw.get("label"), 30), "color": color}

    if kind in ("highlight", "emphasis"):
        target = _id(raw.get("target", raw.get("id")))
        if not target:
            return None
        return {"type": "highlight", "target": target, "color": "yellow" if color == "white" else color}

    return None


def sanitize_board(raw) -> list[dict] | None:
    """The shapes the client can draw, or None when the step has no board."""
    if not isinstance(raw, list):
        return None
    shapes = [s for s in (_shape(r) for r in raw[:MAX_SHAPES_PER_STEP * 2]) if s]
    return shapes[:MAX_SHAPES_PER_STEP] or None


def forced(topic: str) -> tuple[bool, str]:
    """'whiteboard: <topic>' asks for the board whatever is on screen."""
    text = (topic or "").strip()
    if text.lower().startswith("whiteboard:"):
        return True, text[len("whiteboard:"):].strip()
    return False, text
