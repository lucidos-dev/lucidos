# 0652: Unattended data writes stay out of workspace memory, and a spend cap holds background calls until it resets

- **Status**: Accepted
- **Date**: 2026-11-10

Record: `docs/plans/2026-11-10-unattended-writes-and-spend-cap-holds.md`.
Amends [ADR 0362](0362-tree-memory-module-and-the-home-thread.md) (which
artifact writes are workspace leaves) and
[ADR 0640](0640-compactor-breaker-per-lane.md) (what the breaker counts).

## Context

One day of Tree compaction in a busy workspace showed two leaks.

**Scheduled data files fill the workspace tree.** 69% of the day's workspace
leaves were artifact writes from scheduled script triggers. Each is a status or
rollup JSON file, rewritten every 20 to 60 minutes. Each leaf sends up to 30,000
characters to the compactor. ADR 0362 already keeps a trigger thread's
unattended turns out of the workspace tree. It never covered the files those
runs write, and only binaries and bookkeeping files were filtered.

**A spend cap reads as a capacity error.** A gateway answered every call with a
429 whose body was `billing_error`: the daily spend cap was spent, with a reset
time in the message. Every classifier in the provider layer matched the 429. So
each call was retried in place 4 times, the compactor halved its lane, and its
breaker probed every 5 minutes until the reset. Memory extraction and trigger
runs treated it as transient. No surface said "spent until the reset".

## Decision

### 1. An artifact write names its writer, and unattended writers are not leaves

Every `ArtifactCreated`, `ArtifactUpdated` and `ArtifactImported` carries a
typed **artifact writer**, resolved once, when the write lands:

| Writer | Who | Workspace leaf |
|---|---|---|
| `Person` | A device with no app, an upload, a confirmed plugin update | Yes |
| `Thread` | A turn of a thread the workspace memory follows | Yes |
| `App` | An app's UI, through the SDK bridge | Yes |
| `Engine` | Engine machinery, or a caller Lucidos cannot name | Yes |
| `Trigger` | A trigger run nobody joined | No |

`Trigger` covers a script trigger's write, and a write from a trigger thread
before the owner's first message. That is the same line ADR 0362 draws for
turns. A write that carries no writer predates this decision and stays a leaf.

### 2. A spend cap is a typed refusal that holds background calls

The provider layer reads a spend cap from the error's structure, never from
its status alone. It returns a typed `SpendCapReached` with the reset time when
the answer names one. Nothing retries it in place, and no capacity classifier
matches it.

The first refusal starts a **spend cap hold** on that provider. It lasts until
the reset, or one hour when the provider names none. While it lasts:

- background calls to that provider fail fast and send nothing;
- the agent's own turns still send, so a raised cap works at once;
- any successful call to that provider ends the hold;
- the compactor waits for the hold instead of counting breaker failures.

One event announces each hold. The provider row, the Memory row and one
notification show it with its reset time.

## Rationale

**The writer is a fact about the write, and a path is a guess.** A path
heuristic such as "JSON under an app's folder" also catches a user's own JSON
file. Only the write path knows whether a trigger, an app or a person made it.
The origin token already carries the trigger id, and the SDK bridge already
names the app.

**Resolving at write time keeps the log a pure function of events.** Tree
positions must never shift silently (ADR 0362). A read-time rule on the writer
thread's source changes its answer when that thread is deleted. A rule over
old events would shift every existing workspace tree and rebuild almost every
merge, a paid backfill nobody agreed to. A stamped field answers the same way
forever, and old events keep their positions.

**Attended, not "made by an app", is the line.** A trigger runs with nobody
present, so ADR 0362's reason for leaving its turns out holds for its files.
An app's UI writes while a person uses it, and the file can be their own words,
such as a note. Scheduled writes credited to an app are the app's script
triggers, and the `Trigger` writer catches those.

**A spend cap is not congestion.** Halving concurrency, backing off for seconds
or probing every few minutes all assume the next call might succeed. A spent
cap says when it will: at the reset. Until then every background call is noise
in the failure log and a wasted wake-up.

**The hold gates only background work, so ADR 0640's objection does not
apply.** That ADR kept the breaker out of the model call service because a
hidden gate would turn an interactive error into a silent wait. The hold never
gates the agent's turn. A refused call costs no tokens, so the user's own turn
is also the cheapest probe for a raised cap.

## Consequences

- The workspace tree loses lines such as "the notarization status file was
  updated". The files stay in git, readable by the agent, the app and `zoom` on
  any older leaf.
- A trigger's report stays out of workspace memory until the owner writes in
  the trigger thread. The owner's later turns that read the report are leaves
  as usual.
- Workspaces already on Tree keep their older scheduled leaves and summaries.
  Nothing is realigned or rebuilt. A workspace that adopts Tree later still
  backfills its older scheduled writes.
- A script whose origin token no longer verifies, such as a detached child
  that outlived a restart, writes as `Engine` and stays a leaf.
- The app id is attribution, not proof (ADR 0150). A forged one only changes
  the label on a write, and an `App` write is a leaf like any other.
- `llm::spend_cap::SpendCapReached` joins the blessed error types. The
  compactor, the model call service and the trigger runner branch on it.
- The ChatGPT plan's limit pause (ADR 0636) stays as it is, since OpenAI asks
  that interactive plan requests pause too. The compactor reads its
  `LimitReached` as a spend cap, so neither counts as a breaker failure.
- A hold lives in engine memory. A restart forgets it, and the first refused
  call starts it again.

## Alternatives considered

- **Filter by path.** Rejected. It guesses where the write path knows, and it
  hides a user's own file that happens to match.
- **Filter by thread source at read time.** Rejected. A delete changes its
  answer and shifts positions outside the delete's own rebuild.
- **Apply the filter to old events too.** Rejected for this change. It rebuilds
  nearly every workspace merge in every Tree workspace, which spends the user's
  money without asking. A confirmed, opt-in prune can follow.
- **Drop app UI writes as well.** Rejected. An app's UI writes while a person
  uses it, and its files can be what they wrote.
- **Treat a spend cap as a capacity error with a longer wait.** Rejected. It
  still halves the lane, retries in place and probes on a clock that ignores
  the reset the provider already named.
- **Gate every call, the agent's turns included.** Rejected. The user who just
  raised the cap would be locked out until the hold expired.
- **Keep the hold only in the compactor.** Rejected. Memory extraction, titles
  and trigger runs hit the same cap and need the same answer.
