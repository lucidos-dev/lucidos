# 0376: A thread create with no text and no image is refused at /chat/stream; follow-ups stay the agents' to judge

- **Status**: Accepted
- **Date**: 2026-10-06

## Context

A coding-agent thread spawned another with
`lucidos spawn-thread --message "$(cat brief.md)"`. A write hook had blocked
`brief.md`, so `cat` read nothing and the message was empty. `chat_submit`
created the thread. Only then did `run_direct_agent` refuse the empty input
(ADR 0272), and the thread showed nothing but "The reply failed". The caller got
a thread link back and took it as success.

ADR 0272 had rejected "reject an empty message at `chat_submit`". Its reason
holds: the coding-agent invariant belongs to the spawn, and a follow-up's
contract is not at stake. That alternative was about every message, though, and
the failure here is about creates.

## Decision

`chat_submit` refuses a create with no text and no image with a 400, before
the thread is created. A create is a request with no started thread behind it,
which includes a draft's first send. Only a resolved image counts: a stored
hash whose blob is missing is dropped before any agent sees it. `lucidos spawn-thread` refuses the same
case before it sends anything. A follow-up on a started thread is unchanged,
and `require_agent_input` stays where ADR 0272 put it.

## Rationale

- **No agent can start a thread from nothing.** The Lucidos Agent and both
  coding agents need input to take a turn. An empty create is dead on arrival,
  whichever agent it targets.
- **The caller must get the error.** A spawner sees the 400 and its message. A
  refusal after the create only reaches the user, as a dead thread, while the
  caller sees a link.
- **The app already refuses it.** The composer will not send an empty draft, so
  no UI path changes.

## Consequences

- Narrows ADR 0272's third rejected alternative: the create is refused at
  `chat_submit`, every other message is not.
- An empty first send to a draft gets a 400 and the draft stays a draft.
- The `chat_empty_message_is_rejected` API e2e test pins the 400. It used to
  accept a 200.

## Alternatives considered

- **Refuse in the CLI only.** It fixes `spawn-thread`, but any other HTTP caller
  can still create a dead thread.
- **Refuse every empty message at `chat_submit`.** ADR 0272 already rejected
  that: a follow-up is the agents' to judge, and the coding agent already
  refuses one loudly.
- **Delete the dead thread after the agent refuses it.** The caller still gets a
  link to something that no longer exists, and the refusal still never reaches
  it.
