# 0381: Every model call goes through one model call service, locked in by a call token; a call no thread caused records on the home thread; proxied model calls record too

- **Status**: Accepted
- **Date**: 2026-10-07

Amends [ADR 0242](0242-every-model-call-records-its-cost.md). Record:
`docs/plans/2026-10-07-one-model-call-service-records-every-call.md`.

## Context

ADR 0242 said every model call records its cost, and enforced it with a
per-file source audit. The audit passed while real spend went unrecorded.

**The Tree compactor dropped its artifact calls.** A workspace artifact leaf,
and a merge whose span held only artifact leaves, had no thread to record
on. `AuxCapture::for_thread(None)` then emitted nothing. On the dev
workspace that was about 14,000 of roughly 300,000 backfill calls, about 5%
of Tree spend. The backfill estimate's measured seconds per call missed them
too. The compactor file mentions `AuxCapture`, so the per-file audit was
satisfied.

The same `None` dropped three more callers:

- the Classic memory indexer's artifact extraction;
- a `recall/find` from the API or CLI with no thread;
- a title suggestion whose thread id did not parse.

**Two more gaps sat outside the audit's view.** Web search sends a model on
every backend and recorded nothing, since `.search(` was not a shape the
audit looked for. And the credentialed proxy forwards apps' and scripts'
model calls without parsing them. On dev, the jev-browser plugin reported its
own TypeSafe calls as a domain event to work around that. Its Vertex calls
through the same proxy were counted nowhere.

No artifact event named the thread that wrote it: 0 of 14,340 on dev.

## Decision

**One model call service makes every model call, and records it.**
`engine::model_call` holds it. An auxiliary call goes through
`AuxCapture::chat`, `judge`, `generate` or `search`. The agent's own turn goes
through `turn_chat`. Spend that arrives outside a provider call (voice, a
coding agent's side question, the proxy) goes through
`AuxCapture::record_usage`.

**A call token locks it in at compile time.** Each billable provider method
takes a `CallToken`, and only `llm::metered` can make one. Every function
there hands the call's cost to a `CostSink` before returning, and the
engine's sinks are the service. So a model call that records nothing does not
compile.

**A call no thread caused records on the home thread.** It is created hidden
while the home thread switch is off. A capture with no anchor is no longer
expressible.

**Artifact events name the thread that wrote them**, as an optional
`writer_thread_id`. An artifact leaf and an artifact's memory extraction
record on that thread, while it still exists.

**The proxy records the model calls it forwards**, from the usage block in
the reply. A builtin provider proxy always counts, and an `apis.json` entry
counts when its host is a model provider's. The row lands on the thread whose
subprocess made the call, else on the home thread. The proxy drops the
caller's `Accept-Encoding` toward a model provider, since the engine's client
decodes no compressed reply and the parse must read the body.

**A deadline bounds the provider call, never its record.**
`AuxCapture::until` hands the deadline to the metered call, so a call that
answered in time keeps its answer while its row is written.

## Rationale

**A type beats a scan.** The audit asked whether a file mentioned the
recording type, which is not whether each call recorded. The token makes the
only path to a provider one that records. The compiler, not a reviewer, then
catches a new call in an unread file, which is how every gap so far arrived.

**The home thread is the honest anchor for workspace-level work.** It is the
workspace's one thread that never ends and reaches every thread (ADR 0362).
Nothing can delete it, so a row recorded there never lands on a vanished id.
The writer thread is more precise where one exists. Matching an old artifact
to a thread by timestamp would be a guess, and a guess is not an anchor.

**The record outlives its caller.** `AuxCapture` emits on a spawned task that
it awaits, so a timeout around a call can never cancel the row. ADR 0242's
rule, that a deadline covers the call and never the capture, now holds by
construction.

**The lock sits in `llm`, the sinks in `engine`.** `llm` may not depend on
`engine`, a rule its own test enforces. So the token and the metered calls
live in `llm::metered`, and the engine supplies the sinks that write rows.

**The service makes the call; it does not choose the model.** `AuxCall` and
the router still resolve which model a purpose runs on. Once only the service
can call a provider, holding one is harmless, so moving resolution behind it
would add a forwarding layer and no guarantee.

## Consequences

- The per-file audit, its shape list and its empty exemption table are gone.
  Two tests replace them: every billable trait method takes the token, and no
  engine source mints the eval token. `the_only_embedders_run_in_process`
  stays.
- Two purposes join: `web_search` and `proxy`. `proxy` adds the
  `AuxModelSource` arm `CallerChosen`: the caller names the model, so no
  preference applies.
- A workspace with artifacts gets a hidden home thread the first time a
  threadless call records. It is one more, empty, tree in the Tree backfill
  count. Turning the home thread switch on brings back that same thread.
- Every auxiliary chat row now carries its tier and wall-clock time, not only
  the compactor's.
- Tree spend on the dev workspace rises by about 5%, and the Token Cost app
  shows web search and proxied calls for the first time. None of it is new
  cost; all of it was reported as zero.
- Spend before this change stays unreported. The startup backfill grows no
  arm, on ADR 0242's reasoning.
- **Workspace content that reported its own proxied spend now counts it
  twice.** The jev-browser plugin's `JevCallCompleted` duplicates the proxy's
  new rows in the Token Cost rollup until it stops emitting, or the rollup
  stops reading it. Both live in a workspace's `apps/`, not in this repo.
- **Outside the token, by design:**
  - Coding-agent turns record from their own stream output, with their own
    producer and sections.
  - Recording in the Vertex relay would count Claude Code's calls twice, so
    the relay records nothing.

## Alternatives considered

**A threadless `ContextCaptured`, as ADR 0242 drafted.** Dropped again, now on
the merits: every call has a thread to land on, and a row keyed on no thread
breaks the thread-scoped event shape every reader relies on.

**Keep the per-file audit, and add `.search(` to its shapes.** It would have
caught web search. It would not have caught the compactor, whose file already
mentioned the recording type, and a scan is only as good as its shape list.

**A wrapper call with an optional capture.** A helper that records "when given
a capture" recreates the hole the moment a caller passes none. Every call
takes a capture, and a test with no database uses a test-only one that
discards.

**Move provider resolution behind the service as well.** Rejected above: no
new guarantee, and a wider refactor of every call site's model choice.

**Attribute old artifact rows by timestamp or an adjacent `DataFileWritten`.**
Rejected: a near-match is a guess. They record on the home thread.

**The token in `engine::model_call`.** Simpler to read, but it breaks the rule
that `llm` does not depend on `engine`.
