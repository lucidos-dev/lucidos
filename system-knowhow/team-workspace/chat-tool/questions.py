#!/usr/bin/env python3
"""Mirror a gate question into the chat thread its root thread came from.

Copy to data/triggers/chat-questions/scripts/run.py. Fires on
UserQuestionAsked. A thread whose root thread no chat message started is
skipped. See system-knowhow/team-workspace/chat-tool.md.
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
question = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
# A permission request is Lucidos asking the owner for authority, not a gate.
# The retired single-use card's key still marks a card raised before ADR 0688.
engine_card = question.get("permission_request") or question.get("owner_approval")
wanted = thread_id and not engine_card
keyed = keyed_by(thread_id, "chat") if wanted else None
if keyed:
    _root, (channel, thread_ts) = keyed
    options = [o.get("label", "") for o in question.get("options") or []]
    lines = [f"*The factory has a question:* {shorten(question.get('question'), 800)}"]
    if options:
        lines.append("Options: " + ", ".join(f"`{o}`" for o in options if o))
    lines.append(f"Reply in this thread, or <{thread_url(LUCIDOS_URL, thread_id)}|answer in Lucidos>.")
    subprocess.run(
        ["lucidos", "proxy", PROXY, "/chat.postMessage", "-X", "POST",
         "-H", "Content-Type: application/json; charset=utf-8",
         "-d", json.dumps({"channel": channel, "thread_ts": thread_ts, "text": "\n".join(lines)}),
         "--fail"],
        check=True,
    )
