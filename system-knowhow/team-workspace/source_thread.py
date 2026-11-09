"""Find the source address of a work item, for the outbound script triggers.

Copy to data/scripts/team-workspace/source_thread.py. The intake trigger's
continue key starts with the source name the translation script wrote, so
the key alone says which source a work item came from and where to reply.
See system-knowhow/team-workspace.md.
"""
import json
import subprocess

# A family deeper than this is a loop in the data, not a real work item.
MAX_DEPTH = 20


def events(event_type, thread_id):
    """The thread's events of one type, newest first."""
    out = subprocess.run(
        ["lucidos", "events", "query", "--type", event_type,
         "--thread-id", thread_id, "--limit", "1000"],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(out or "[]")


def family_root(thread_id):
    """The work item root: follow each thread's first message up to its parent."""
    for _ in range(MAX_DEPTH):
        received = events("MessageReceived", thread_id)
        parent = received[-1]["payload"].get("parent_thread_id") if received else None
        if not parent:
            return thread_id
        thread_id = parent
    return thread_id


def continue_key(root):
    """The continue key the first keyed fire gave the root, as a list."""
    for row in reversed(events("TriggerStarted", root)):
        key = row["payload"].get("continue_key")
        if key:
            return json.loads(key) if isinstance(key, str) else key
    return None


def keyed_by(thread_id, source):
    """(root, address) when `source` started this thread's work item, else None.

    The address is the rest of the continue key after the source name.
    """
    root = family_root(thread_id)
    key = continue_key(root)
    if not key or str(key[0]) != source:
        return None
    return root, [str(part) for part in key[1:]]


def thread_url(lucidos_url, thread_id):
    """A link that opens the thread in the team workspace."""
    return f"{lucidos_url.rstrip('/')}/#thread={thread_id}"


def shorten(text, limit):
    """At most `limit` characters, cut at a word."""
    text = (text or "").strip()
    if len(text) <= limit:
        return text
    return text[:limit].rsplit(" ", 1)[0] + " …"
