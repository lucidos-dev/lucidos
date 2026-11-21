#!/usr/bin/env python3
"""Post a short outcome to the issue when the root thread answers.

Copy to data/triggers/issue-tracker-outcomes/scripts/run.py. Fires on
ResponseGenerated. Sub-thread replies and unkeyed threads are skipped, so
only the root's answer reaches the issue, PR link included when it has one.
See system-knowhow/team-workspace/issue-tracker.md.
"""
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.environ.get("LUCIDOS_WORKSPACE", "."), "data", "scripts", "team-workspace"))
from source_thread import keyed_by, shorten, thread_url  # noqa: E402

PROXY = "issue-tracker"
LUCIDOS_URL = "https://lucidos.example.com/factory"

thread_id = os.environ.get("TRIGGER_EVENT_THREAD_ID")
response = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
text = response.get("text") or ""
keyed = keyed_by(thread_id, "issue-tracker") if thread_id and text.strip() else None
if keyed and keyed[0] == thread_id:
    _root, (repo, number) = keyed
    body = f"{shorten(text, 1200)}\n\nThe whole thread: {thread_url(LUCIDOS_URL, thread_id)}"
    subprocess.run(
        ["lucidos", "proxy", PROXY, f"/repos/{repo}/issues/{number}/comments",
         "-X", "POST", "-H", "Content-Type: application/json",
         "-d", json.dumps({"body": body}), "--fail"],
        check=True,
    )
