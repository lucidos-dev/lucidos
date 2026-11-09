#!/usr/bin/env python3
"""Poll a chat tool and emit a ChatMessageReceived domain event per new message.

Copy to data/triggers/chat-translate/scripts/run.py and run it every minute.
The worked example reads Slack's Web API. It polls rather than takes a
webhook, so it needs no public address. A new message that mentions the bot starts a work
item. Every later reply in its chat thread continues it.
See system-knowhow/team-workspace/chat-tool.md.
"""
import json
import os
import subprocess
import time

CHANNELS = ["C0000000000"]
# The bot's own user id. Its own posts come back in the history.
BOT_USER = "U0000000000"
PROXY = "chat"
# How long a chat thread keeps being polled for replies.
FOLLOW_SECS = 14 * 24 * 3600
STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state", "poll.json")
# The teammates who may start and steer work: a JSON list of the emails on
# their persons. Anyone else in the channel, a guest included, is a stranger.
PEOPLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state", "people.json")


def api(path):
    out = subprocess.run(["lucidos", "proxy", PROXY, path, "--fail"],
                         check=True, capture_output=True, text=True).stdout
    body = json.loads(out or "{}")
    if not body.get("ok"):
        raise SystemExit(f"{path}: {body.get('error')}")
    return body


def load_state():
    try:
        with open(STATE) as f:
            return json.load(f)
    except FileNotFoundError:
        return {"cursors": {}, "threads": {}, "emails": {}}


def save_state(state):
    os.makedirs(os.path.dirname(STATE), exist_ok=True)
    with open(STATE, "w") as f:
        json.dump(state, f)


def email_of(state, user):
    if user not in state["emails"]:
        profile = api(f"/users.info?user={user}")["user"].get("profile") or {}
        state["emails"][user] = (profile.get("email") or "").strip().lower() or None
    return state["emails"][user]


def teammates():
    try:
        with open(PEOPLE) as f:
            return {e.strip().lower() for e in json.load(f)}
    except FileNotFoundError:
        return set()


def emit(state, channel, thread_ts, message):
    """Emit one message from a teammate, then save the cursor it advanced, so
    a failure later in the run never emits it twice."""
    email = email_of(state, message["user"])
    if email not in TEAM:
        save_state(state)
        return
    event = {
        "summary": f"Chat message in {channel}",
        "source": "chat",
        "channel": channel,
        "thread_ts": thread_ts,
        "ts": message["ts"],
        "text": message.get("text") or "",
        "author_email": email,
    }
    subprocess.run(["lucidos", "events", "emit", "ChatMessageReceived",
                    "--payload", json.dumps(event)], check=True)
    save_state(state)


def from_a_person(message):
    return message.get("user") and message["user"] != BOT_USER and not message.get("subtype")


TEAM = teammates()
state = load_state()
now = time.time()
for channel in CHANNELS:
    cursor = state["cursors"].get(channel)
    if cursor is None:
        # The first run only starts the clock, so old history starts nothing.
        state["cursors"][channel] = str(now)
        continue
    history = api(f"/conversations.history?channel={channel}&oldest={cursor}&limit=200")
    for message in sorted(history["messages"], key=lambda m: float(m["ts"])):
        state["cursors"][channel] = message["ts"]
        top_level = message.get("thread_ts", message["ts"]) == message["ts"]
        if top_level and from_a_person(message) and f"<@{BOT_USER}>" in message.get("text", ""):
            state["threads"][f"{channel}/{message['ts']}"] = message["ts"]
            emit(state, channel, message["ts"], message)

for key, seen in list(state["threads"].items()):
    channel, thread_ts = key.split("/")
    if now - float(thread_ts) > FOLLOW_SECS:
        del state["threads"][key]
        continue
    replies = api(f"/conversations.replies?channel={channel}&ts={thread_ts}&oldest={seen}")
    for message in sorted(replies["messages"], key=lambda m: float(m["ts"])):
        if float(message["ts"]) <= float(seen):
            continue
        state["threads"][key] = message["ts"]
        if from_a_person(message):
            emit(state, channel, thread_ts, message)

save_state(state)
