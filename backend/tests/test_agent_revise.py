"""Revising a draft: the earlier text goes into the prompt verbatim, the revised
text comes back as a type_text action, and what was drafted is remembered.

Run from backend/:  python -m unittest discover tests
"""
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from app.worker import tasks  # noqa: E402

DRAFT = "Hi Jordan,\n\nThanks for reaching out. I will have the report ready by Friday afternoon.\n\nBest,\nSam"
REVISED = "Hi Jordan,\n\nThe report will be ready Friday.\n\nBest,\nSam"


class FakeLLM:
    provider = "gemini"
    model = "fake"

    def __init__(self, reply):
        self.reply = reply
        self.prompts = []

    def analyze_text_sync(self, prompt, use_search=False):
        self.prompts.append(prompt)
        return json.dumps(self.reply)

    def analyze_sync(self, image_bytes, prompt, use_search=False):
        return self.analyze_text_sync(prompt)


def run_task(llm, **kw):
    recorded = []
    args = dict(task_description="make it shorter", session_id="s1", thread_id="t1", clipboard_text="")
    args.update(kw)
    with mock.patch.object(tasks, "get_llm", return_value=llm), \
            mock.patch.object(tasks, "get_history", return_value=[]), \
            mock.patch.object(tasks, "record_exchange", lambda *a: recorded.append(a)):
        result = tasks.run_agent_task.run(**args)
    return result, recorded


class ReviseTests(unittest.TestCase):
    def reply(self):
        return {
            "answer_text": "Shortened the middle paragraph.",
            "clipboard_write": None,
            "proposed_action": {"action_type": "type_text", "text": REVISED, "description": "Type the shorter reply"},
        }

    def test_previous_draft_is_in_the_prompt_verbatim(self):
        llm = FakeLLM(self.reply())
        run_task(llm, previous_draft=DRAFT)
        self.assertIn(DRAFT, llm.prompts[0])
        self.assertIn("COMPLETE revised text", llm.prompts[0])
        self.assertLess(llm.prompts[0].index(DRAFT), llm.prompts[0].index("Task: make it shorter"))

    def test_no_revision_text_when_not_revising(self):
        llm = FakeLLM(self.reply())
        run_task(llm)
        self.assertNotIn("COMPLETE revised text", llm.prompts[0])

    def test_revised_text_comes_back_as_the_action(self):
        result, _ = run_task(FakeLLM(self.reply()), previous_draft=DRAFT)
        self.assertEqual(result["proposed_action"]["action_type"], "type_text")
        self.assertEqual(result["proposed_action"]["text"], REVISED)

    def test_typed_draft_is_remembered_in_full_for_the_next_turn(self):
        _, recorded = run_task(FakeLLM(self.reply()), previous_draft=DRAFT)
        thread_id, session_id, mode, query, remembered = recorded[0]
        self.assertEqual((thread_id, session_id, mode, query), ("t1", "s1", "agent", "make it shorter"))
        self.assertIn(f"Drafted text: {REVISED}", remembered)

    def test_a_first_draft_proposed_as_typing_is_remembered_too(self):
        reply = self.reply()
        _, recorded = run_task(FakeLLM(reply), task_description="reply saying I will be late")
        self.assertIn("Drafted text:", recorded[0][4])

    def test_draft_is_not_duplicated_when_clipboard_and_action_carry_the_same_text(self):
        reply = self.reply()
        reply["clipboard_write"] = REVISED
        _, recorded = run_task(FakeLLM(reply), previous_draft=DRAFT)
        self.assertEqual(recorded[0][4].count("Drafted text:"), 1)


if __name__ == "__main__":
    unittest.main()
