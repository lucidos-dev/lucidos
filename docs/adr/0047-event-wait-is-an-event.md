# 0047: A thread's event wait is an event; the dispatcher is a cache rebuilt at boot

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

A *trigger* can subscribe to events. A *thread* could not. So when an agent turn
needed to wait for a state change the engine already publishes as an event, it
had no primitive to express that and fell back to polling.

The measured case, chat thread *Apply Idle Changes and Release*: a 180-iteration
shell loop sampling two CLI calls every 20 s, followed by five `bash_output`
drains interleaved with filler work, because the agent could not simply stop.
**20 LLM calls, 4,250,960 cumulative context tokens, peaking at 244,473 per
call.** Each drain re-entered at full context to learn nothing had happened yet.
The predicate that loop encodes was already two events the engine knew about the
instant they happened, and told nobody who was waiting. It was also racy in a
way the loop could not fix: two CLI calls sampled 20 s apart can observe a torn
state that never existed, and can miss a transition that opens and closes inside
one interval.

## Decision

A thread parks on an **event wait** registered by the terminal `await_event` LLM
tool, and:

1. **The persisted `EventWaitStarted` event IS the wait.** No table. The
   dispatcher's live set (`engine/event_wait/`) is a cache rebuilt from the
   event store at boot.
2. **Delivery is one-shot.** The first matching event resolves the wait and
   consumes it. `await_event` is a rendezvous, not a stream.
3. **A watermark plus a catch-up scan closes the restart gap.** Each wait
   records the event `sequence` at registration; registration and boot recovery
   run the same forward scan.
4. **One predicate language.** `EventSubscription` and its matcher moved out of
   `triggers/` to `core::event_subscription` and are shared verbatim, so a
   `condition` that fires for a trigger fires for a wait.
5. **A user message detaches a wait rather than cancelling it.**
6. **Every resolution writes a wake anchor, and a stranded one is re-driven at
   boot.** A resolution is immediately followed by exactly one of two events:
   the paired `await_event` `ToolResult` when the wait was still attached, or a
   `UserPromptInjected` carrying the payload as prose when it had detached.

## Rationale

**The wait is an event because a wake already is.** ADR 0011 rejected a second
persisted representation for the child-thread fan-in wake, on the grounds that
the event store already holds the fact and a table would be a second thing to
keep true. An event wait is the same shape of problem: the wait must survive a
restart (Engine Statelessness), and the only state the dispatcher holds is
derivable from `EventWaitStarted` rows with no later resolution carrying their
`wait_id`.

**One-shot follows from the tool-call shape, it was not chosen freely.** A tool
call has exactly one tool result, so N deliveries for one `tool_use_id` are not
expressible in the message array at all. Stating it as a semantic ("first match
wins, later matches are ignored because the wait no longer exists") is honest
about a constraint the transport already imposes.

**The watermark closes two gaps with one mechanism**, which is why it is worth a
field rather than a special case: events that landed while the engine was down,
and the live race between emitting `EventWaitStarted` and the dispatcher
inserting its cache entry. Both are "events after sequence N that I have not
matched yet."

**Attachment is derived, never stored.** The elegant property of the design is
that the model's `await_event` tool call stays unpaired across the park, so the
delivered event arrives as its own `tool_result` and the model resumes
mid-thought with no exchange boundary. Whether that slot is still open is
exactly "does a `ToolResult` for this call exist", which the provider's own
message array already answers. A stored `attached: bool` could only disagree
with it.

**The wake anchor is what makes the last restart gap recoverable.** The three
mechanisms above cover a wait that is still live at restart. They deliberately
cannot help a wait that RESOLVED and whose turn never ran, because re-arming it
would be wrong: the delivery is already recorded, and the boot rebuild correctly
skips it. What is missing there is only the turn. Giving each resolution a
recognisable second event turns that into a query the same shape as
`refire_unprocessed_child_completions`, which recovers the identical gap for the
child-thread fan-in: a resolution whose thread has no later event *other than*
its own anchor never woke, so re-drive it. Bounded by construction, since the
re-driven turn's own events become the thread's later word.

It also settles a question the design otherwise leaves open, which is how a
detached delivery reads in the transcript. `UserPromptInjected` is already an
exchange-start event on the frontend, so using it as the detached anchor makes
the wake render as the new turn it genuinely is, with no new event type and no
payload-aware exception in the exchange grouper.

**Re-arming a subscription is not re-running a turn.** Worth stating because the
crash-safety gate in CLAUDE.md says a crashed engine keeps the manual Continue
affordance rather than auto-resuming. That rule is about work that was RUNNING
and might have caused the crash. A parked thread holds no tokio task and no LLM
call, so the boot rebuild only restores a watch and the thread resumes when its
event genuinely lands. The lost-wake sweep is the one path that does re-drive
work at boot, and it is safe for the reason above.

**Detach beats cancel on a user message.** The first design cancelled the wait
on any `MessageReceived`, mirroring the question park. That is hostile in the
obvious case: forty minutes into waiting for a release, the user asks "how's it
going?" and the wait silently evaporates, leaving a thread that looks subscribed
and is not. Detaching costs a coarser resume for the interrupted delivery (a new
exchange rather than a seamless tool result) and buys a subscription that
survives ordinary conversation. It also makes "this thread is working AND
watching for something" a representable state, which is what the subscription
indicator shows.

## Consequences

**Kept.** No migration and no new table. A parked thread survives any restart,
because the boot rebuild re-derives the live set. The two dispatch paths cannot
disagree on a `condition`, pinned by a parity table run through both. A parked
thread occupies zero Thread Queue slots, so N waiting threads cannot deadlock
the pool against the work that would wake them.

**Given up.** No streaming waits: "react to every X" stays a trigger. No
`await_event` for coding-agent threads in v1. No CLI or SDK parity, and
deliberately so: `await_event` is intrinsically an agent-turn primitive, since
there is no turn for `lucidos await-event` to park (an ADR 0018 exemption, noted
in the capability manifest module docs).

**Costs paid deliberately.** The catch-up scan is a query per registration and
per boot-recovered wait, which is fine at the scale of "a handful of parked
threads" and would need rethinking at thousands. The deadline sweep polls a
10-second tick, so an expiry can land up to 10 s late. Attachment is a small
query per delivery rather than a field read.

**A guard that must exist before the feature.** A parked thread has no
terminator by design and a deliberately dangling tool call, which is precisely
the shape both restart sweeps read as a crashed turn. The preserve guard
(`attached_event_wait_exists_sql`) therefore landed before the tool did, so a
parked thread can never exist without it.

## Alternatives considered

**A blocking call that holds the loop open** (`await_event(..., block=true)`,
the `bash_output(wait_secs)` shape). Rejected: status stays `running`, so
`reconcile_user_slot` keeps the Thread Queue slot for the whole wait. N blocked
threads occupy the pool while the very work that would emit their events queues
behind them, which is a genuine deadlock and not merely waste. It also pins a
tokio task, the full message array and the LLM context for the duration, and
dies on restart with no persisted record of what it wanted, violating Engine
Statelessness. This is why `bash_output`'s ceiling is 300 s; a release wait is
tens of minutes.

**A per-thread trigger that posts back into the thread.** Expressible today, and
precisely the workaround being deleted. It costs a persisted trigger row per
wait (orphaned if the thread dies), a whole extra LLM turn in a *different*
thread just to route the message, and the result arrives as a fresh
`MessageReceived` that starts a new exchange with no tool-call pairing. The
waiting thread has no distinct state, so it reads as finished. "Wake me after 20
minutes anyway" needs a second, cron trigger.

**Keep polling.** Measured above: 20 LLM calls, 4.25 M context tokens, a 20 s
sampling race, and a documented inline-XML failure (the model emitted a
`bash_output` poll as inline XML text mid-turn and terminated with raw markup,
which is why `engine/inline_tool_call_repair.rs` exists).

**A wake-question workaround** (`ask_user_question` purely to get resumed).
Rejected: it makes the human the scheduler. It lights the needs-attention badge
and pushes a notification for something the engine already knows, and blocks
Apply while the card is open.

**A `thread_event_waits` table.** Rejected per ADR 0011's precedent: a second
persisted representation of a wake that the event store already records, with
its own migration, its own drift risk, and no capability the rebuild lacks.

**A stored `attached` flag.** Rejected: the tool-use pairing in the message
array is the thing that actually decides the delivery shape, and any cached
mirror of it is a second source of truth for a fact the first one already
answers exactly.

**Cancelling the wait on a user message** (the original S6). Rejected during
implementation review; see the Rationale above.

## Amendment, 2026-08-06: the transcript surface gets lighter, not richer

The two-surface split above stands, but the weights it assigned were wrong on a
phone, and this is what changed.

**The transcript card is gone.** The record of a park was a boxed
`.step-note-card` spanning the response width, which is the weight the codebase
reserves for an inline *affordance* (the command-guard checkpoint, which carries
an Undo). A park is one action the agent took. It now renders as one line in the
step list, subject first, and the details it used to spell out are the
indicator's job: the indicator is where the live countdown and Stop already
were, and the ADR's own reasoning for the split says the indicator is primary.
Nothing moved to a surface that did not already own it. The one affordance kept
in the transcript is the jump to a matched event, because a resolved wait has
left the indicator and the link is its only route there.

**The detached wake shows the event, not the prompt.** The design leaves the
wake's `UserPromptInjected.text` as the model's prompt, which necessarily spells
the matched payload out as pretty-printed JSON, and the client rendered that
text verbatim as markdown. On a 393px viewport a `CodingAgentIdled` delivery was
a screen and a half of raw JSON. `UserPromptInjected` therefore gained an
optional `delivered_event_id` pointing back at the `EventWaitDelivered` that
produced it, which already carries `event_type` and `payload` as fields; the
transcript renders the event's name with the payload folded away, and falls back
to the prose when the link is absent or its target is outside the loaded window.
An id rather than a copy, so there is still exactly one structured record of the
delivery, and the client never parses engine-authored prose back apart.

Neither half changes the wake mechanics: the anchor is still a
`UserPromptInjected`, still an exchange-start event, and still what the lost-wake
sweep recognises.
