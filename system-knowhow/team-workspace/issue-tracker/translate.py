#!/usr/bin/env python3
"""Turn an issue tracker delivery into an IssueActivityReceived domain event.

Copy to data/triggers/issue-tracker-translate/scripts/run.py. The worked
example reads a GitHub `issues` or `issue_comment` delivery. For another
tracker, change `activity()` and `email_of()` only: the event it emits stays
the same. See system-knowhow/team-workspace/issue-tracker.md.
"""
import json
import os
import subprocess

# The bot account the factory posts as. Its own comments come back as
# deliveries, and translating them would answer the factory's own questions.
BOT_LOGIN = "example-factory-bot"
# The teammates who may start work: tracker login to the email on their person.
# Any other login is a stranger. Its deliveries start nothing.
PEOPLE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "state", "people.json")


def load_people():
    try:
        with open(PEOPLE) as f:
            return {k.lower(): v for k, v in json.load(f).items()}
    except FileNotFoundError:
        return {}


def email_of(login):
    """A teammate's email, or None for a stranger. A public profile email
    proves nothing: anyone can open an issue on a public repository."""
    return load_people().get(login.lower())


def activity(headers, body):
    """(kind, issue, text, login) for a delivery worth a thread, else None."""
    kind = headers.get("X-GitHub-Event")
    issue = body.get("issue") or {}
    if kind == "issues" and body.get("action") == "opened":
        return "opened", issue, issue.get("body") or "", body["sender"]["login"]
    if kind == "issue_comment" and body.get("action") == "created":
        comment = body["comment"]
        return "comment", issue, comment.get("body") or "", comment["user"]["login"]
    return None


raw = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
headers = raw.get("headers") or {}
body = raw.get("payload") if isinstance(raw.get("payload"), dict) else raw
found = activity(headers, body)
if found:
    kind, issue, text, login = found
    email = None if login.lower() == BOT_LOGIN.lower() else email_of(login)
    # A stranger, or the bot itself, starts nothing.
    if email:
        event = {
            "summary": f"{login} {kind} on issue #{issue['number']}",
            "source": "issue-tracker",
            "repo": body["repository"]["full_name"],
            "number": issue["number"],
            "title": issue.get("title") or "",
            "url": issue.get("html_url") or "",
            "kind": kind,
            "text": text,
            "author_email": email.strip().lower(),
        }
        subprocess.run(
            ["lucidos", "events", "emit", "IssueActivityReceived",
             "--payload", json.dumps(event)],
            check=True,
        )
