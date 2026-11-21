# 0701: Context ceiling hands off at a safe boundary

- **Status**: Accepted
- **Date**: 2026-11-15
- **Amends**: [0629](0629-engine-enforced-context-ceiling.md)

## Context

ADR 0629 checked the context ceiling only at clean turn ends and exempted a
session's first turn. Long turns, Stops, redirects and shutdowns could skip it.
A checkpoint successor grew from 60k to 654k without recording a crossing.

| Context size | Boundary | Why the old rule missed it |
|---|---|---|
| 60k to 365k | Shutdown aborts | No clean idle; still the opening turn |
| 385k | User Stop | Canceled turn; first counted idle |
| 474k | User Stop | Canceled turn |
| 474k to 654k | Long Continue turn | No turn end before the checkpoint |

Four of eight observed threads with captures past 400k recorded no crossing.
One reached 770k. The deferred mid-turn signal in ADR 0629's plan is now needed.

## Decision

**Record the crossing on live main-session capture; deliver at the next safe
boundary without interrupting a tool or turn.** `ContextCeilingReached` records
an owed handoff once per session, independently of delivery.
Restored captures and child-agent usage cannot record a crossing.

**Replace the first-turn exemption with an opening guard.** A session whose
first capture already exceeds the ceiling is exempt, since handing off might
not shrink it. A session that grows past the ceiling acts even in its opening
turn. Unknown input-token readings never fire.

### Safe delivery

A driver that reports `can_steer` can receive the re-entry during a running turn.
The agent stops at a safe point and leaves running tasks for its successor.
Otherwise delivery waits for the turn end.

Unread input, an open question or native permission prompt, or a handoff block
defers steering. The engine checks again on subsequent captures and at turn end.
An automation pause asks the owner before delivery; a newer pause invalidates
approval that has not yet been delivered.

Delivery remains once per session across both paths. A steered input read inside
the user's turn preserves that turn's actor, read decision and wake decision.
An input read after the preceding Result opens an engine turn and skips those
decisions. Forwarding alone does not prove the agent has read it.

A safe steer refusal queues delivery until idle without interrupting.
An unknown delivery receipt retires the input without resending; turn end can
settle a brief. Stop withdraws an unread steer before interrupting the agent.

### Terminal settlement

| Turn end | Owed handoff |
|---|---|
| Clean | Re-entry ahead of later input |
| Follow-up redirect | Re-entry ahead of the follow-up |
| User Stop | Brief at idle; no engine turn |
| Failure | Brief at idle |
| Shutdown abort | Preserve debt; next real spawn records the brief |

A re-entry that ends without a narrative still records a brief.
No brief replaces a re-entry while its question or permission card is open.
Later ordinary input waits for the successor from the crossing onward.
Card answers and approved ceiling input retain their delivery channels.
The successor starts on the next input, as ADR 0629 specifies.

## Rationale

A durable capture-time record survives every terminal kind and restart.
Separating it from delivery avoids losing crossings while the engine waits for
a safe boundary. Steering uses the driver's existing input protocol and never
cuts a running tool. The opening guard prevents loops without exempting long
opening turns.

Stop remains authoritative under ADR 0696: no automatic re-entry runs afterward.
A brief preserves continuity until the next human input without spending another
agent turn. Running tasks, children, event waits and the worktree belong to the
thread and survive the handoff.

## Consequences

- Every coding-agent thread and workspace uses this policy on macOS and Linux,
  across packaged, headless and source installs.
- Claude Code and Codex app-server use steering when their driver supports it.
  Drivers without steering retain turn-end delivery and durable settlement.
- Lucidos Agent threads and unknown input-token readings remain excluded.
- The ceiling value, scopes, raise authority, checkpoints and estimate do not
  change. No new event type or payload field is needed.
- Overshoot can still grow while input or cards defer delivery, or while a
  driver without steering waits for turn end. The engine never interrupts.

## Alternatives considered

- **Keep checking only clean turn ends.** The observed shutdowns, Stops and long
  turns show that these boundaries can remain absent for hours.
- **Interrupt immediately at the ceiling.** It could cut a build, take or lock
  and lose work.
- **Exempt every first turn.** It exempts successors that grow far past the
  ceiling during their opening work.
- **Run a narrative turn after Stop.** It violates the user's Stop authority.
- **Treat a forwarded input as read.** Claude Code may read it after Result;
  that would misclassify the preceding user turn and suppress its decisions.
