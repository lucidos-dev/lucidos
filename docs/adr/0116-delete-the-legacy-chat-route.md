# 0116: The legacy POST /api/v1/chat route is deleted, not hardened

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

Two routes wrote the same mutation. `POST /api/v1/chat` and
`POST /api/v1/chat/stream` both reached `process_message_with_steps`, and both
persisted a `MessageReceived` carrying the caller's `mode`. Only the streaming
one applied the full gate list.

The bare route ran three of the seven checks its sibling ran. It skipped the
subprocess-origin gate, the mode and repo continuity lock, and Thread Queue
admission, and it passed `origin: None`, so its event carried no actor. Those
three gaps were not a single oversight. They accumulated, one gate at a time, as
each was added to `chat_submit` and not to `chat`.

ADR 0050 met this route while closing a different hole. It hardened what it had
to, noted the route "has no caller anywhere in the tree", and flagged the
deletion for a separate decision. This is that decision.

## Decision

Delete the route, the `chat` handler, the `ChatResponse` body type and the
now-unreachable `thread_summary_exists` query. `POST /api/v1/chat/stream` is the
only way to submit a chat turn over HTTP.

## Rationale

**Nothing called it.** A sweep of the whole tree found no caller: not the
frontend, the SDK, the `lucidos` CLI, `engine::http::workspace_client`, the API
e2e suite, or the browser e2e suite. Every one of them uses `/chat/stream`. The
three remaining mentions were prose using the path as an example string, and
they now name the route that exists.

**Two gate lists over one mutation is the defect, not the missing gates.** The
gaps could have been filled. Filling them leaves the structure that produced
them: two entry points to one write, where every future gate has to be added
twice, and where forgetting the second one is silent. The route had already been
half-hardened once, which is how it ended up with three of the seven.

**The gap it left was real.** `origin: None` means the persisted event names
nobody, so a turn written through this route was indistinguishable from one
nobody sent. Skipping the subprocess gate let an authenticated coding-agent
subprocess write into a thread it does not own. That is what ADR 0043 and
ADR 0050 exist to prevent on the sibling path.

## Consequences

- One entry point for a chat turn over HTTP, so one gate list to maintain.
- `subprocess_chat_legitimate` keeps exactly one non-test caller.
- A caller that hardcoded `POST /api/v1/chat` outside this repository gets a 404
  from the `/api/v1` fallback. Nothing in the tree does, and the route was never
  documented in `system-knowhow/`, so no published contract breaks.
- The response shape it alone returned (`{response, steps}`, the whole answer in
  one blocking body) is gone. `/chat/stream` returns `{event_id}` and the answer
  arrives over SSE. Anything wanting the blocking shape would have to rebuild
  it, deliberately, with the gates attached.

## Alternatives considered

- **Harden it: add the three missing gates and build a `MessageOrigin`.** The
  option ADR 0050 left open. Rejected: it pays for a route nobody calls, and it
  preserves the two-list structure that produced the drift. The next gate added
  to `chat_submit` would face the same fork, with the same silent failure mode.
- **Keep it, and redirect to `/chat/stream`.** Rejected: the two responses have
  incompatible shapes. A redirect would answer `{event_id}` to a caller waiting
  for `{response, steps}`, which is a breakage wearing a compatibility costume.
- **Deprecate it first: log a warning and remove it later.** Rejected: a
  deprecation window buys time for callers to migrate, and there are no callers.
  The warning would only ever fire for a caller that does not exist.
- **Keep the handler, drop the route registration.** Rejected outright. That is
  dead code carrying a comment that promises it is nearly live. The no-dead-code
  rule exists because the next reader re-wires it, trusting the comment.
