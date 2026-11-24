#!/usr/bin/env python3
"""Mirror a gate question to the issue its root thread came from.

Copy to data/triggers/issue-tracker-questions/scripts/run.py. Fires on
UserQuestionAsked. A thread whose root thread no issue started is skipped.
See system-knowhow/team-workspace/issue-tracker.md.
"""
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.environ.get("LUCIDOS_WORKSPACE", "."), "data", "scripts", "team-workspace"))
from source_thread import keyed_by, shorten, thread_url  # noqa: E402

PROXY = "issue-tracker"
# The team workspace's own address, workspace path included.
LUCIDOS_URL = "https://lucidos.example.com/factory"

thread_id = os.environ.get("TRIGGER_EVENT_THREAD_ID")
question = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
# A permission request is Lucidos asking the owner for authority, not a gate.
# The retired single-use card's key still marks a card raised before ADR 0688.
engine_card = question.get("permission_request") or question.get("owner_approval")
wanted = thread_id and not engine_card
keyed = keyed_by(thread_id, "issue-tracker") if wanted else None
if keyed:
    _root, (repo, number) = keyed
    options = [o.get("label", "") for o in question.get("options") or []]
    lines = [f"**The factory has a question:** {shorten(question.get('question'), 800)}"]
    if options:
        lines.append("Options: " + ", ".join(f"`{o}`" for o in options if o))
    lines.append(f"Reply here, or answer in Lucidos: {thread_url(LUCIDOS_URL, thread_id)}")
    subprocess.run(
        ["lucidos", "proxy", PROXY, f"/repos/{repo}/issues/{number}/comments",
         "-X", "POST", "-H", "Content-Type: application/json",
         "-d", json.dumps({"body": "\n\n".join(lines)}), "--fail"],
        check=True,
    )
