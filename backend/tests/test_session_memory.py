"""Conversation memory: shared storage keyed by thread, trimmed, formatted
for prompts, and never allowed to break a request.

Run from backend/:  python -m unittest discover tests
"""
import asyncio
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.services import session_memory as mem  # noqa: E402


class FakeRedis:
    """Just the list commands session_memory uses; async twin below."""

    def __init__(self):
        self.lists = {}
        self.ttl = {}

    def rpush(self, key, value):
        self.lists.setdefault(key, []).append(value)

    def ltrim(self, key, start, end):
        items = self.lists.get(key, [])
        self.lists[key] = items[start:] if start < 0 else items[start:end + 1]

    def expire(self, key, secs):
        self.ttl[key] = secs

    def lrange(self, key, start, end):
        items = self.lists.get(key, [])
        return items[start:] if end == -1 else items[start:end + 1]


class AsyncFakeRedis(FakeRedis):
    async def rpush(self, key, value):
        return super().rpush(key, value)

    async def ltrim(self, key, start, end):
        return super().ltrim(key, start, end)

    async def expire(self, key, secs):
        return super().expire(key, secs)

    async def lrange(self, key, start, end):
        return super().lrange(key, start, end)


class BrokenRedis:
    def __getattr__(self, name):
        raise ConnectionError("redis is down")


def request(**kw):
    base = dict(session_id="s1", thread_id="", session_duration_secs=0, app_name="", extra_context="")
    base.update(kw)
    return SimpleNamespace(**base)


class MemoryTests(unittest.TestCase):
    def setUp(self):
        self.sync = FakeRedis()
        self.asyn = AsyncFakeRedis()
        self._saved = (mem._sync_client, mem._async_client)
        mem._sync_client, mem._async_client = self.sync, self.asyn

    def tearDown(self):
        mem._sync_client, mem._async_client = self._saved

    def test_key_prefers_thread_over_session(self):
        self.assertEqual(mem.memory_key("t1", "s1"), "mem:t1")
        self.assertEqual(mem.memory_key("", "s1"), "mem:s1")

    def test_record_then_read_round_trip(self):
        mem.record_exchange("t1", "s1", "direct", "what is this", "a cat")
        got = mem.get_history("t1", "s1")
        self.assertEqual(got, [{"mode": "direct", "query": "what is this", "answer": "a cat"}])

    def test_kept_at_most_max_exchanges_and_expiry_refreshed(self):
        for i in range(mem.MAX_EXCHANGES + 5):
            mem.record_exchange("t1", "s1", "direct", f"q{i}", f"a{i}")
        stored = self.sync.lists["mem:t1"]
        self.assertEqual(len(stored), mem.MAX_EXCHANGES)
        self.assertEqual(json.loads(stored[-1])["query"], f"q{mem.MAX_EXCHANGES + 4}")
        self.assertEqual(self.sync.ttl["mem:t1"], mem.TTL_SECS)

    def test_agent_worker_and_api_share_one_conversation(self):
        # The point of moving to Redis: the Celery worker (sync) and the API
        # process (async) used to keep separate memories.
        self.asyn.lists = self.sync.lists  # one Redis, two client flavors
        mem.record_exchange("t1", "s1", "agent", "draft a reply", "Hi, thanks!")
        got = asyncio.run(mem.aget_history("t1", "s1"))
        self.assertEqual(got[0]["mode"], "agent")
        asyncio.run(mem.arecord_exchange("t1", "s1", "direct", "make it shorter", "Thanks!"))
        self.assertEqual([h["mode"] for h in mem.get_history("t1", "s1")], ["agent", "direct"])

    def test_async_record_and_read(self):
        asyncio.run(mem.arecord_exchange("", "s9", "explain", "triangles", "step one step two"))
        got = asyncio.run(mem.aget_history("", "s9"))
        self.assertEqual(got[0]["mode"], "explain")

    def test_unknown_mode_is_stored_as_direct(self):
        mem.record_exchange("t", "s", "weird", "q", "a")
        self.assertEqual(mem.get_history("t", "s")[0]["mode"], "direct")

    def test_long_values_are_capped_on_write(self):
        mem.record_exchange("t", "s", "direct", "q" * 5000, "a" * 5000)
        item = mem.get_history("t", "s")[0]
        self.assertEqual(len(item["query"]), mem.STORED_QUERY_LEN)
        self.assertEqual(len(item["answer"]), mem.STORED_ANSWER_LEN)

    def test_corrupt_entries_are_skipped(self):
        self.sync.lists["mem:t"] = ["not json", json.dumps({"mode": "direct", "query": "q", "answer": "a"}), "[1]"]
        self.assertEqual(len(mem.get_history("t", "s")), 1)

    def test_redis_down_never_raises(self):
        mem._sync_client = BrokenRedis()
        mem._async_client = BrokenRedis()
        mem.record_exchange("t", "s", "direct", "q", "a")
        self.assertEqual(mem.get_history("t", "s"), [])
        asyncio.run(mem.arecord_exchange("t", "s", "direct", "q", "a"))
        self.assertEqual(asyncio.run(mem.aget_history("t", "s")), [])


class FormatTests(unittest.TestCase):
    def test_empty_history_adds_nothing(self):
        self.assertEqual(mem.format_history([]), "")
        self.assertEqual(mem.compose_context(request(), []), "")

    def test_latest_answers_keep_more_text_than_older_ones(self):
        long_answer = "word " * 400
        history = [{"mode": "direct", "query": f"q{i}", "answer": long_answer} for i in range(4)]
        lines = mem.format_history(history).splitlines()[1:]
        self.assertEqual(len(lines), 4)
        self.assertLess(len(lines[0]), len(lines[-1]))  # oldest clipped hardest
        self.assertLess(len(lines[1]), len(lines[2]))
        self.assertLessEqual(len(lines[-1]), mem.RECENT_ANSWER_LEN + 120)

    def test_drafted_text_in_latest_agent_result_survives_for_make_it_shorter(self):
        draft = "Hi Sam, thanks for the update. I will send the report tomorrow morning."
        history = [{"mode": "agent", "query": "reply to this", "answer": f"Drafted.\nDrafted text: {draft}"}]
        self.assertIn(draft, mem.format_history(history))

    def test_mode_labels_distinguish_what_happened(self):
        history = [
            {"mode": "explain", "query": "triangles", "answer": "x"},
            {"mode": "agent", "query": "open notepad", "answer": "y"},
        ]
        text = mem.format_history(history)
        self.assertIn("Asked for an explanation of", text)
        self.assertIn("Gave an agent task", text)

    def test_compose_includes_dwell_extra_context_and_history(self):
        req = request(app_name="Chrome", session_duration_secs=600, extra_context="User interrupted step 2.")
        out = mem.compose_context(req, [{"mode": "direct", "query": "q", "answer": "a"}])
        self.assertIn("Chrome for 10 minutes", out)
        self.assertIn("User interrupted step 2.", out)
        self.assertIn('Asked: "q"', out)
        self.assertLess(out.index("Chrome"), out.index("interrupted"))
        self.assertLess(out.index("interrupted"), out.index("Conversation so far"))

    def test_short_dwell_is_not_mentioned(self):
        self.assertEqual(mem.compose_context(request(app_name="Chrome", session_duration_secs=5), []), "")


if __name__ == "__main__":
    unittest.main()
