"""Streaming explain: steps are pulled out of the model's stream as they
finish and sent on one per line, whatever shape the model writes them in.

Run from backend/:  python -m unittest discover tests
"""
import base64
import json
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from fastapi.testclient import TestClient  # noqa: E402

from app.main import app  # noqa: E402
from app.rate_limit import rate_limit  # noqa: E402
from app.routes import analyze  # noqa: E402
from app.services.step_stream import JsonObjectStream, expand_steps  # noqa: E402

PNG_1PX = base64.b64encode(
    bytes.fromhex(
        "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
        "0000000d4944415478da63fcffff3f0300050001fe0d0aed4c0000000049454e44ae426082"
    )
).decode()


class ParserTests(unittest.TestCase):
    def feed_all(self, chunks):
        p = JsonObjectStream()
        out = []
        for c in chunks:
            out.extend(p.feed(c))
        return out

    def test_one_object_per_line(self):
        out = self.feed_all(['{"narration": "a"}\n{"narration": "b"}\n'])
        self.assertEqual([o["narration"] for o in out], ["a", "b"])

    def test_object_split_across_chunks_at_any_boundary(self):
        text = '{"narration": "hello {not a brace} \\" quote", "point": [1, 2]}\n{"narration": "b"}'
        for size in (1, 2, 3, 7, 50):
            chunks = [text[i:i + size] for i in range(0, len(text), size)]
            out = self.feed_all(chunks)
            self.assertEqual(len(out), 2, f"chunk size {size}")
            self.assertEqual(out[0]["narration"], 'hello {not a brace} " quote')

    def test_step_is_emitted_before_the_next_one_starts(self):
        p = JsonObjectStream()
        self.assertEqual(p.feed('{"narration": "first"}\n{"narr'), [{"narration": "first"}])
        self.assertEqual(p.feed('ation": "second"}'), [{"narration": "second"}])

    def test_fences_commas_arrays_and_prose_are_skipped(self):
        text = 'Sure!\n```json\n[\n{"narration": "a"},\n{"narration": "b"}\n]\n```'
        self.assertEqual([o["narration"] for o in self.feed_all([text])], ["a", "b"])

    def test_pretty_printed_objects(self):
        text = '{\n  "narration": "a",\n  "line": [[1, 2], [3, 4]]\n}\n{\n  "narration": "b"\n}'
        self.assertEqual(len(self.feed_all([text])), 2)

    def test_invalid_object_is_dropped_not_fatal(self):
        out = self.feed_all(['{"narration": oops}\n{"narration": "ok"}'])
        self.assertEqual(out, [{"narration": "ok"}])

    def test_wrapper_object_is_expanded(self):
        out = self.feed_all(['{"steps": [{"narration": "a"}, {"narration": "b"}]}'])
        self.assertEqual(len(out), 1)
        self.assertEqual([s["narration"] for s in expand_steps(out[0])], ["a", "b"])
        self.assertEqual(expand_steps({"narration": "a"}), [{"narration": "a"}])


class NormalizeTests(unittest.TestCase):
    def test_point_box_line_and_none(self):
        point = analyze._normalize_step({"narration": "p", "point": [300, 400]})
        self.assertEqual((point.shape, point.x_norm, point.y_norm), ("point", 0.4, 0.3))
        box = analyze._normalize_step({"narration": "b", "box_2d": [100, 200, 300, 400]})
        self.assertEqual((box.shape, box.x_norm, box.y_norm, box.x2_norm, box.y2_norm), ("box", 0.2, 0.1, 0.4, 0.3))
        line = analyze._normalize_step({"narration": "l", "line": [[100, 200], [300, 400]]})
        self.assertEqual((line.shape, line.x2_norm, line.y2_norm), ("line", 0.4, 0.3))
        self.assertIsNone(analyze._normalize_step({"narration": "n"}).shape)

    def test_missing_narration_or_non_dict_is_dropped(self):
        self.assertIsNone(analyze._normalize_step({"point": [1, 2]}))
        self.assertIsNone(analyze._normalize_step({"narration": "   "}))
        self.assertIsNone(analyze._normalize_step("text"))

    def test_malformed_annotation_keeps_the_narration(self):
        step = analyze._normalize_step({"narration": "keep me", "point": ["a", "b"]})
        self.assertEqual(step.narration, "keep me")
        self.assertIsNone(step.shape)

    def test_out_of_range_coordinates_are_clamped(self):
        step = analyze._normalize_step({"narration": "x", "point": [5000, -20]})
        self.assertEqual((step.x_norm, step.y_norm), (0.0, 1.0))


class FakeLLM:
    def __init__(self, pieces=None, boom_after=None):
        self.pieces = pieces or []
        self.boom_after = boom_after
        self.prompt = None

    async def analyze_stream(self, image_bytes, prompt):
        self.prompt = prompt
        for i, piece in enumerate(self.pieces):
            if self.boom_after is not None and i >= self.boom_after:
                raise RuntimeError("stream broke")
            yield piece


def payload(**kw):
    base = {
        "screenshot_base64": PNG_1PX,
        "cursor_position": {"x_norm": 0.5, "y_norm": 0.5},
        "screen_resolution": {"width": 100, "height": 100},
        "active_window_title": "Chrome",
        "query_text": "pythagoras",
        "session_id": "s1",
        "thread_id": "t1",
        "timestamp": "2026-01-01T00:00:00Z",
    }
    base.update(kw)
    return base


class RouteTests(unittest.TestCase):
    def setUp(self):
        app.dependency_overrides[rate_limit] = lambda: None
        self.recorded = []

        async def fake_record(thread_id, session_id, mode, query, answer):
            self.recorded.append((thread_id, session_id, mode, query, answer))

        async def fake_context(request):
            return "CONTEXT BLOCK"

        self._patches = [
            mock.patch.object(analyze, "arecord_exchange", fake_record),
            mock.patch.object(analyze, "build_session_context_block", fake_context),
        ]
        for p in self._patches:
            p.start()
        self.client = TestClient(app)

    def tearDown(self):
        for p in self._patches:
            p.stop()
        app.dependency_overrides.clear()

    def post(self, llm, **kw):
        with mock.patch.object(analyze, "get_llm", return_value=llm):
            res = self.client.post("/api/analyze-explain-stream", json=payload(**kw))
        self.assertEqual(res.status_code, 200)
        return [json.loads(line) for line in res.text.splitlines() if line.strip()]

    def test_steps_stream_one_per_line_then_done(self):
        llm = FakeLLM([
            '{"narration": "Step one.", "point": [300, 400]}\n{"narr',
            'ation": "Step two.", "box_2d": [1, 2, 3, 4]}\n',
            '{"narration": "Step three."}',
        ])
        events = self.post(llm)
        self.assertEqual([e["type"] for e in events], ["step", "step", "step", "done"])
        self.assertEqual(events[0]["step"]["narration"], "Step one.")
        self.assertEqual(events[0]["step"]["shape"], "point")
        self.assertEqual(events[3]["count"], 3)
        self.assertIn("JSON Lines", llm.prompt)
        self.assertIn("CONTEXT BLOCK", llm.prompt)

    def test_exchange_is_remembered_once_at_the_end(self):
        self.post(FakeLLM(['{"narration": "One."}\n{"narration": "Two."}']))
        self.assertEqual(self.recorded, [("t1", "s1", "explain", "pythagoras", "One. Two.")])

    def test_model_ignoring_the_format_still_works_via_wrapper(self):
        events = self.post(FakeLLM(['```json\n{"steps": [{"narration": "A."}, {"narration": "B."}]}\n```']))
        self.assertEqual([e["step"]["narration"] for e in events if e["type"] == "step"], ["A.", "B."])

    def test_no_usable_steps_gives_one_spoken_apology(self):
        events = self.post(FakeLLM(["Error communicating with the model: quota"]))
        self.assertEqual([e["type"] for e in events], ["step", "done"])
        self.assertEqual(events[0]["step"]["narration"], analyze.EXPLAIN_FALLBACK_NARRATION)
        self.assertEqual(self.recorded, [])

    def test_stream_failing_midway_keeps_the_steps_already_sent(self):
        events = self.post(FakeLLM(['{"narration": "Kept."}\n', '{"narration": "Lost."}'], boom_after=1))
        self.assertEqual([e["step"]["narration"] for e in events if e["type"] == "step"], ["Kept."])

    def test_stream_failing_before_any_step_says_something_went_wrong(self):
        events = self.post(FakeLLM(["x"], boom_after=0))
        self.assertEqual(events[0]["step"]["narration"], analyze.EXPLAIN_ERROR_NARRATION)

    def test_invalid_image_is_a_400_not_a_stream(self):
        with mock.patch.object(analyze, "get_llm", return_value=FakeLLM()):
            res = self.client.post("/api/analyze-explain-stream", json=payload(screenshot_base64="abcde"))
        self.assertEqual(res.status_code, 400)

    def test_old_one_shot_route_still_works(self):
        class OneShot:
            async def analyze(self, image_bytes, prompt, json_mode=False):
                return '{"steps": [{"narration": "Only step.", "point": [10, 20]}]}'

        with mock.patch.object(analyze, "get_llm", return_value=OneShot()):
            res = self.client.post("/api/analyze-explain", json=payload())
        body = res.json()
        self.assertEqual(body["steps"][0]["narration"], "Only step.")
        self.assertEqual(body["steps"][0]["shape"], "point")


if __name__ == "__main__":
    unittest.main()
