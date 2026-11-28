#!/usr/bin/env python3
"""Post a short outcome into the chat thread when the root thread answers.

Copy to data/triggers/chat-outcomes/scripts/run.py. Fires on
ResponseGenerated. Sub-thread replies and unkeyed threads are skipped.
See system-knowhow/team-workspace/chat-tool.md.
"""
import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.join(os.environ.get("LUCIDOS_WORKSPACE", "."), "data", "scripts", "team-workspace"))
from source_thread import keyed_by, shorten, thread_url  # noqa: E402

PROXY = "chat"
LUCIDOS_URL = "https://lucidos.example.com/factory"

thread_id = os.environ.get("TRIGGER_EVENT_THREAD_ID")
response = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
text = response.get("text") or ""
keyed = keyed_by(thread_id, "chat") if thread_id and text.strip() else None
if keyed and keyed[0] == thread_id:
    _root, (channel, thread_ts) = keyed
    body = f"{shorten(text, 1200)}\n<{thread_url(LUCIDOS_URL, thread_id)}|The whole thread>"
    subprocess.run(
        ["lucidos", "proxy", PROXY, "/chat.postMessage", "-X", "POST",
         "-H", "Content-Type: application/json; charset=utf-8",
         "-d", json.dumps({"channel": channel, "thread_ts": thread_ts, "text": body}),
         "--fail"],
        check=True,
    )
