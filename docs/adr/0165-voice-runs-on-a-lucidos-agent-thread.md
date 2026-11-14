# 0165: Voice runs on a Lucidos Agent thread, and refuses a coding-agent one

- **Status**: Accepted
- **Date**: 2026-08-30

## Context

Voice shipped with no rule about which agent a call reaches. The doer wakes
`process_message_with_steps` with `use_coding_agent` unset and stamps
`EventChannel::Chat`, so it always woke the Lucidos Agent. Nothing in `voice/`
read a thread's backend.

That was harmless while nothing offered a call anywhere else. But `CallToggle`
renders in every prompt input, coding-agent threads included, and
`api::voice::admit` checked only that the thread exists. So a spoken utterance
on a live Claude Code thread landed as a chat-channel message and started a
Lucidos Agent turn inside it.

`api::chat::validate_thread_continuity` refuses exactly that over HTTP: a thread
is locked to the mode it picked on its first message. The voice path bypasses
the API layer, so the lock never applied.

A user hit the softer half of this. They picked Claude Code in the compose view,
pressed the call control, and reached the Lucidos Agent. The control never read
their pick, because only `sendCompose` binds it.

## Decision

A call runs on a Lucidos Agent thread and refuses every other thread.

Three layers say so, and they are not redundant. The **control** is absent
whenever the resolved destination is a coding agent. **`admit`** refuses the
socket. **The doer** refuses to start a turn.

A live call is exempt from the control's rule, so a caller never loses the
button they ring off with. Moving the destination under a live call ends that
call and says why.

The boundary is temporary. It is a capability we have not built, not one we
have ruled out.

## Rationale

**The doer refusing is the root fix; the rest is how the user meets it.** The
originating layer for "which agent gets woken" is `voice/doer.rs`, and a guard
anywhere else leaves the bug reachable by a path nobody enumerated. The other
two layers exist because a refusal a person only meets as silence is a bad
refusal.

**The three layers cover three different things.** The control covers intent:
it stops a call being placed, and it is the only layer a person actually meets.
`admit` covers the request, refusing the socket to any client. The doer covers
the race, which is real: a compose draft's destination can move while a socket
is already open.

**Only the control can explain itself, and that is why it carries the weight.**
A `WebSocket` hides the handshake's status and body, so `admit`'s sentence
reaches the log and the tests and never a browser: our client shows its generic
`CALL_REFUSED` whatever the engine wrote. So the refusal a person meets is the
control being absent beside the picker that brings it back.

**Hidden rather than disabled**, following `CallToggle`'s existing rule for the
voice-off case: a dead button is a thing to wonder about. The way back is
already on screen, because the destination picker sits in the same row.

**The gate is the resolved destination, not the stored channel.**
`effectiveCodingAgentBackend` already answers for all three shapes: a started
thread, a composing draft, and the fresh compose view with no draft yet. Reading
it means the control and the call cannot disagree, and the rule is not restated
a third time.

**A refused utterance is written down and spoken.** `wake` reports whether it
took the utterance. A refusal writes nothing, so the call loop records a
`SpokenMessageReceived` and has the talker say that nothing started. Skip that
and a `WorkDelegated` sits beside no record of what was said, while the caller
waits for an answer that is never coming.

**Trigger threads keep voice.** The rule is which agent holds a thread, never
how the thread began. A trigger thread's turns run the Lucidos Agent, so
`ThreadType::from_source` reads it as a chat thread and a call reaches it.

## Consequences

- Voice is a Lucidos Agent capability, and the UI says so before the engine has
  to.
- A coding-agent thread cannot be talked to. That is a real loss, and it is the
  thing this defers rather than solves.
- The doer stays the thread's own agent by construction, so widening this later
  is a branch in one function rather than a new path.
- `ThreadType::from_source` gives `"claude_code"` one home. A fourth
  hand-written copy of that literal is how the spellings drift.

## Alternatives considered

**Support voice on coding-agent threads.** Planned in full and deferred by the
maintainer. The reconnaissance survives in
`docs/plans/2026-08-30-a-call-runs-on-a-lucidos-agent-thread.md`, including the
four things that would need deciding. Two of them are why it is not free.

A coding-agent session has no passive channel for a spoken aside: everything on
`msg_tx` becomes a prompt the agent acts on. A draft's picks are also unwritten
until its first keystroke. Pressing the control without typing therefore leaves
the server holding the mode but not the repo.

**Hide the control and stop there.** Rejected. Hiding a button is not a
guarantee: it says nothing to another client, and it cannot cover a destination
moved while a socket is open.

**Refuse in the engine and leave the control visible.** Rejected for the
opposite reason. It is honest and it works, but it offers a button whose only
outcome is a toast, and one that cannot even say why: the browser never sees
the engine's message.

**Disable the destination picker during a call.** Rejected. Ending the call is
one rule where this would be two. A disabled picker also hides the way out at
the exact moment the reader wants it.

**Let a live call continue after the destination moves.** Rejected. The doer
would refuse the next utterance anyway, so the caller would keep a call that can
no longer do anything.
