"""Whiteboard shapes: whatever the model writes is reduced to what the client
draws, kept on the canvas, and attached to the step.

Run from backend/:  python -m unittest discover tests
"""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.routes import analyze  # noqa: E402
from app.services.whiteboard import forced, sanitize_board, MAX_SHAPES_PER_STEP  # noqa: E402


class SanitizeTests(unittest.TestCase):
    def test_no_board(self):
        self.assertIsNone(sanitize_board(None))
        self.assertIsNone(sanitize_board("box"))
        self.assertIsNone(sanitize_board([]))
        self.assertIsNone(sanitize_board([{"type": "star"}]))

    def test_box_is_kept_on_the_canvas(self):
        [box] = sanitize_board([{"type": "box", "id": "a", "x": 900, "y": 550, "w": 400, "h": 300, "label": "A"}])
        self.assertEqual((box["x"], box["y"]), (900, 550))
        self.assertEqual((box["w"], box["h"]), (100, 50))
        self.assertEqual(box["color"], "white")

    def test_aliases_and_defaults(self):
        [box] = sanitize_board([{"type": "Rectangle", "x": 10, "y": 10, "width": 120, "height": 60, "color": "BLUE"}])
        self.assertEqual((box["type"], box["w"], box["h"], box["color"]), ("box", 120, 60, "blue"))
        self.assertIsNone(box["id"])

    def test_circle_radius_fits_the_canvas(self):
        [circle] = sanitize_board([{"type": "circle", "id": "c", "x": 30, "y": 300, "r": 200}])
        self.assertEqual(circle["r"], 30)

    def test_text_needs_text(self):
        self.assertIsNone(sanitize_board([{"type": "text", "x": 1, "y": 1}]))
        [text] = sanitize_board([{"type": "text", "x": 1, "y": 1, "text": "  E = mc²  ", "size": "xl"}])
        self.assertEqual((text["text"], text["size"]), ("E = mc²", "m"))

    def test_arrow_ends(self):
        [by_id] = sanitize_board([{"type": "arrow", "from": "a", "to": "b"}])
        self.assertEqual((by_id["from"], by_id["to"]), ("a", "b"))
        [by_spot] = sanitize_board([{"type": "arrow", "from": {"x": -5, "y": 20}, "to": [2000, 30]}])
        self.assertEqual(by_spot["from"], {"x": 0, "y": 20})
        self.assertEqual(by_spot["to"], {"x": 1000, "y": 30})
        self.assertIsNone(sanitize_board([{"type": "arrow", "from": "a"}]))
        self.assertIsNone(sanitize_board([{"type": "arrow", "from": "a", "to": "a"}]))

    def test_highlight(self):
        [h] = sanitize_board([{"type": "highlight", "target": "a"}])
        self.assertEqual(h, {"type": "highlight", "target": "a", "color": "yellow"})
        self.assertIsNone(sanitize_board([{"type": "highlight"}]))

    def test_bad_numbers_drop_the_shape_not_the_board(self):
        board = sanitize_board([
            {"type": "box", "x": "left", "y": 10},
            {"type": "box", "x": 10, "y": 10, "label": "ok"},
        ])
        self.assertEqual([b["label"] for b in board], ["ok"])

    def test_long_strings_and_too_many_shapes_are_cut(self):
        board = sanitize_board([{"type": "box", "x": 1, "y": 1, "label": "x" * 500}] * 50)
        self.assertEqual(len(board), MAX_SHAPES_PER_STEP)
        self.assertEqual(len(board[0]["label"]), 40)


class StepTests(unittest.TestCase):
    def test_board_is_attached_to_the_step(self):
        step = analyze._normalize_step({"narration": "n", "board": [{"type": "box", "x": 1, "y": 1}]})
        self.assertEqual(step.board[0]["type"], "box")

    def test_board_wins_over_a_screen_annotation(self):
        step = analyze._normalize_step({"narration": "n", "point": [500, 500], "board": [{"type": "text", "x": 1, "y": 1, "text": "t"}]})
        self.assertIsNone(step.shape)
        self.assertIsNotNone(step.board)

    def test_unusable_board_keeps_the_screen_annotation(self):
        step = analyze._normalize_step({"narration": "n", "point": [500, 500], "board": [{"type": "star"}]})
        self.assertEqual(step.shape, "point")
        self.assertIsNone(step.board)


class ForcedTests(unittest.TestCase):
    def test_whiteboard_prefix(self):
        self.assertEqual(forced("whiteboard: how DNS works"), (True, "how DNS works"))
        self.assertEqual(forced("WhiteBoard:recursion"), (True, "recursion"))
        self.assertEqual(forced("how a whiteboard works"), (False, "how a whiteboard works"))

    def test_prompt_mentions_the_board_either_way(self):
        class Req:
            active_window_title = "x"
            query_text = "whiteboard: DNS"
        forced_prompt = analyze._build_explain_prompt(Req(), "", streaming=True)
        self.assertIn("ignore the screen", forced_prompt)
        self.assertIn("'DNS'", forced_prompt)
        Req.query_text = "DNS"
        normal = analyze._build_explain_prompt(Req(), "", streaming=True)
        self.assertNotIn("ignore the screen", normal)
        self.assertIn("WHITEBOARD", normal)


if __name__ == "__main__":
    unittest.main()
