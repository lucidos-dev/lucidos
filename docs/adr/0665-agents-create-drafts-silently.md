# 0665: Agents create draft threads silently

- **Status**: Accepted
- **Date**: 2026-11-12

## Context

An agent that wanted to hand the user a ready-to-send message had one route:
`navigate_ui(target="new-chat", prompt=...)`. It unfocused the thread the user
was reading and focused the composer. Two agents doing it at once fought over
the screen. Nothing let an agent prepare a draft and leave the user where they
were.

## Decision

One engine service creates a new composing thread with a seeded draft. It
reuses the saved-draft seams: a `ThreadStarted` seed beside `recovered_compose`,
one projection transaction for the compose columns, and `ThreadComposeChanged`
after commit. It emits no navigation, no message and no queue row, and it starts
no session. `lucidos threads create-draft` and `POST /api/v1/threads/drafts`
call it. No `threads` tool action does: its schema bills every chat turn, as
ADR 0664 found for permissions.

## Rationale

- **No second draft store.** The draft is an ordinary saved draft, so the
  composer, the drafts list and the send feature read it unchanged.
- **The user keeps their place.** A draft is an offer, so taking focus for it is
  the wrong default. The drafts list and the compose broadcast make it visible.
- **Honest attribution.** The actor is the agent's origin. The row's
  `initiator` is `system` and `drafted_by_thread_id` names the agent's thread,
  which the drafts read reports as `drafted_by`. So the user can tell an
  agent's draft from their own.
- **A retry never clobbers an edit.** The thread id derives from the calling
  thread and a retry key, and the seed only inserts.

## Consequences

- Sending stays a separate feature with its own rules. A created draft only
  keeps the exact shape that feature accepts.
- Attachments are images, because the composer stores images only.

## Alternatives considered

- **A `navigate_ui` flag that skips focus.** Rejected: navigation is the wrong
  verb, and the draft would still live only in the client.
- **A separate agent-drafts table.** Rejected: a second store drifts from the
  composer and needs its own send path.
- **Create and send in one call.** Rejected: an unsent draft is the point. Send
  needs the owner's say, which belongs to its own feature.
