# 0242: Every engine model call emits a ContextCaptured

- **Status**: Accepted
- **Date**: 2026-09-22

## Context

The Token Cost app reads one input: `ContextCaptured` rows carrying a provider
`usage` block. A model call emitting none is spend nobody can see.

Six were emitting none. Two of the three engine sites that call TypeSafe Jev,
and four calls on the agent's own chat model:

| Call | Weight |
|---|---|
| The command guard's judge, on the Jev path | one call per ambiguous command |
| The command guard's judge, on the chat path | the same, and it is the default backend |
| The agent's `judge` tool | one per call, batches of up to a hundred questions |
| The `execute_intent` sub-loop | up to a hundred rounds per invocation |
| The `correct_memory` verdict | one per correction |
| The `import_file` artifact summary | one per imported file |

ADR 0107 predicted the first of these in its own Consequences: "The command
guard's judge is an auxiliary call with NO purpose, so it emits no
`ContextCaptured` and its tokens go unaccounted. Giving it a purpose is the
obvious follow-up."

The pattern under all six is the same. `ContextPurpose` was grown one variant at
a time, by whoever happened to be adding a call, and nothing failed when a call
arrived without one. The invariant existed in prose and in nobody's build.

## Decision

**Every model call the engine makes records what it cost**, through
`AuxCapture`, under a `ContextPurpose` of its own.

Five purposes were added: `CommandJudge`, `JudgeTool`, `IntentLoop`,
`MemoryCorrection` and `ArtifactSummary`. There are no exemptions.

**A purpose that reads no model preference names which kind it is.** ADR 0107's
"one purpose per auxiliary model preference" was enforced over an
`Option<AuxModelPrefs>`, where `None` meant `Turn`. Two of the new purposes read
no preference for two unlike reasons, so the `Option` became `AuxModelSource`
with an arm each:

| Arm | Means | Members |
|---|---|---|
| `Turn` | Not an auxiliary call at all. | `Turn` |
| `BackendPinned` | One backend, so nothing to choose. | `JudgeTool` |
| `AgentModel` | Runs the agent's own chat model, so no preference and no reachable budget. | `IntentLoop`, `MemoryCorrection`, `ArtifactSummary` |
| `Preferences` | A pair the user sets. | the other eight |

The invariant test matches on that enum with no wildcard and asserts the
membership of every arm.

**The enforcement is a rule plus a source audit.** The rule is a bullet in
`.claude/rules/rust.md`, beside the EventBus one it is the sibling of. The audit
is `every_file_that_calls_a_model_records_the_cost` in `engine/aux_capture.rs`:
it walks the crate's source for the three provider-call shapes and fails on a
file that mentions neither `AuxCapture` nor `ContextCaptured`. Its `UNRECORDED`
table is empty.

**A Jev row carries the model the response named.** `parse_response` was
dropping the body's `model` field, so a caller could only stamp the alias it
asked for, `jev-latest`. The response says `jev-1.13.0`. The jev-browser plugin
already reads it, so stamping the alias would split one model across two lines
in a cost rollup.

## Rationale

**Per FILE, not per call, is what makes the audit hold.** A reviewer does spot a
second uncaptured call inside a file that already captures: the recording line
is right there in the diff. Nobody spots a model call appearing in a file they
were not reading, which is exactly how all six got in. A file-level assertion
also survives refactoring, where a per-call one would churn on every move.

**The arms are not bookkeeping.** The old test did assert that only `Turn` read
no preference, so an exemption would not have been silent. It would have been
one line widening that assertion, which reads as noise in a diff. An arm makes
the same edit name the category joined. The answers differ in what Settings
should offer and in what a budget means.

**A timeout covers the provider call, never the capture.** Wrapped around the
whole helper it also wraps the row being written. A call answering just inside
the deadline then loses its accounting, which is the one thing here that cannot
be retried. So each helper takes the deadline and applies it to the call alone.
The command guard needs the two told apart: a timeout declines the chat fallback
and a failure takes it. Its Jev path returns a boxed
`tokio::time::error::Elapsed` the caller downcasts.

**Both command-guard backends stamp one purpose.** The backend is the user's
choice under `judgment_command_guard`, and the job is the same either way.
Capturing only the Jev path would have left the DEFAULT path silent. A workspace
switching to Jev would then read a backend change as a rise in spend.

**The provider reports the size, not the caller.** `JevProvider::ask`
serializes the body, so it knows what went out. The reference site was counting
its own state, missing the questions riding with it. That is the kind of quiet
undercount a second caller copies.

## Consequences

- Adding a `ContextPurpose` variant fails the build until it declares its arm
  and its budget. `budget_for` lost its wildcard arm to make the second half
  true: ADR 0107 promised it, and a `_ => SHORT_CALL_BUDGET` had quietly taken
  it back. Adding a model call in a new file fails the audit until it records.
- The Token Cost app needs no change. Its rollup keys on `payload->>'model'`,
  and every model the new purposes stamp already has a pricing card.
- The new rows are visible spend that was always being spent. A workspace with
  `judgment_command_guard = jev` will see a TypeSafe line appear, and one that
  uses `execute_intent` will see its agent's model line rise. Neither is new
  cost, and both were previously reported as zero.
- The `judge` tool's timeout moved into `aux_purpose` as `JUDGE_TOOL_BUDGET`,
  so the number a reader finds beside the purpose is the number the call runs
  under. Its per-attempt cap is unchanged at 60s.
- `UNCAPTURED_CALL_BUDGET` is gone. It existed only to give the command guard a
  budget without a purpose, and its doc comment documented the bug.
- **Embeddings stay outside**, and `.embed(` is deliberately absent from the
  audit's shape list. The only production `EmbeddingProvider` is
  `FastEmbedProvider`, which runs in-process and bills nothing. What keeps that
  true is `the_only_embedders_run_in_process`, which fails the moment a
  different implementor appears.
- The startup backfill grows no arm for any new purpose, on ADR 0107's
  reasoning: reconstructing past calls under a purpose that did not exist would
  relabel the past. Spend made before this change stays unreported.

## Alternatives considered

**Fix only the two Jev sites, as first asked.** It was the reported bug.
Rejected once the survey found four more. The question behind the report is
"what is this costing me", and a half-swept answer is a wrong one that looks
right.

**Leave the command guard's chat path alone.** It is the older, better-known
path. Rejected because it is the DEFAULT one: capturing the opt-in backend
alone would make a backend switch look like new spend.

**Exempt the preference-less purposes from the invariant.** One line, and the
test keeps passing. Rejected because an exemption list is how the invariant
stopped covering the command guard in the first place. A named arm costs the
same and cannot be joined by accident.

**Stamp `JEV_DEFAULT_MODEL` everywhere and skip parsing the response.** Simpler,
and the alias does have a pricing card. Rejected because the plugin path already
stamps the version, so the two halves of one model's spend would sit on separate
lines forever.

**File the intent sub-loop under `main_llm` / `Turn`.** The sub-loop is the
agent working on the user's behalf, so the turn line is arguably right. Rejected
on three counts. `main_llm` promises a section breakdown this row has none of,
and the modal reads Turn rows to draw drift across one turn. A trigger scoped to
`purpose: "turn"` also asked for the agent's own turns. A body-less auxiliary
row tells all three the truth.

**A workspace-level cost event for a call with no thread.** Drafted for the
artifact summary, which looked like a threadless rebuild sweep. Dropped on the
facts: `summarize_artifact` has one caller, the `import_file` tool, so it runs
inside a turn and has a thread like everything else. Worth recording because the
misreading was load-bearing for a whole alternative design.

**A `scripts/check-*.sh` instead of a Rust test.** It would run in `/harden`
Phase 4.5 beside the other deterministic gates. Rejected because the check needs
the crate's own notion of which traits are providers, and a cargo test already
runs on every Rust diff. A shell script scanning Rust for trait methods would
drift from the traits it scans for.
