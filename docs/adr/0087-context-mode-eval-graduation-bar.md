# 0087: The context-mode eval is a paired two-workspace sequence scored on planted probes, not on whether the task got done

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

ADR 0085 ships an experimental context mode behind a workspace preference. From
round 2 of a thread it stops sending memory recall and the conversation history.
It also drops the resume tool pairs at every turn boundary. The model is
expected to note what matters into an extended `todo_write` block, and to
recover the rest through a tool call.

0085 makes a task eval the bar for graduating that flag to the default, and
states three constraints. Two workspaces are seeded identically and run the same
sequence of threads, one arm lean. A workspace built from scratch has no memory
and no history, so the first task measures nothing. An agent is not
deterministic, so the unit is tasks times repeats times two arms.

Its own open list then says the eval has no ADR and no plan, and calls it the
largest unwritten piece. Nothing defines the task set, the seeding, the repeat
count or the scoring.

The workspace supplies the numbers this has to be sized against. Over 30 days
the dev workspace ran 286 chat threads. Rounds per thread averaged 65, with a
median of 32 and a 90th percentile of 149. A round reads 114,644 cached tokens
on average and writes 8,353.

The busiest tools are `run_bash`, `edit_file`, `run_python`, `read_file`,
`ask_user_question`, `grep_files`, `todo_write`, `load_knowhow`, `events` and
`triggers`. The workspace holds 25 apps, 28 triggers and 14 knowhow entries.

Two facts about the sections shape everything below. Memory recall is a
cross-thread channel, and the conversation history is a within-thread one. They
fail differently. An eval that cannot attribute a loss to one of them says the
flag is bad, without saying what to fix.

## Decision

**The eval is a fixed sequence of 12 threads, run end to end in two seeded
workspaces.** Planted probes score it, on facts established earlier in the same
sequence. Fifteen numbered commitments follow, then the task set, then the bar.

1. **The sequence run is the unit, not the task.** Twelve threads run in a fixed
   order, and later threads depend on named earlier ones. The twelve outcomes
   inside one run are correlated by construction. So the analysis treats the run
   as a cluster and never as twelve independent samples.
2. **The primary endpoint is a recall census, not task success.** The last
   thread asks for a handover document. The score is the proportion of the
   register's established facts stated correctly in it. Task success is a
   secondary endpoint. This sharpens 0085's "scored on whether the task got
   done". A task that got done while quietly losing a detail is the exact
   failure the flag risks.
3. **Every dependency is a planted probe carrying a recovery tier.** Tier 1 is a
   fact in a file, and tier 2 a fact in an event payload. Tier 3 is a fact from
   an earlier thread's conversation, and tier 4 one stated earlier in the same
   turn. 0085 deletes tiers 3 and 4. Tiers 1 and 2 are controls, and a
   regression there means the agent stopped looking rather than stopped
   remembering.
4. **A probe resolves to one of seven values.** `in-prompt`, `from-notes`,
   `recovered`, `asked`, `lost-silent`, `lost-loud`, `void`. Pass is the first
   three. `asked` and `lost-loud` are visible failures, costing a round or a
   human but not amnesia. `lost-silent` is the failure 0085 exists to rule out,
   and it is counted on its own line.
5. **A probe must have a tempting wrong default.** The task prompt never
   restates the fact, and an agent that lacks it has a plausible alternative to
   reach for. A probe whose only failure is producing nothing measures loudness,
   not retention.
6. **Scoring is programmatic first.** Assertions read the workspace filesystem,
   the projections and the event store. An LLM judge scores only the two probes
   no assertion can express, and otherwise triages which threads a human reads.
7. **A blinded human read of a stratified 10% sample is the only instrument that
   sees an unplanted regression.** It is a sample, it is never the primary
   metric, and the reader is not told the arm.
8. **A failed precondition voids the downstream probes rather than failing
   them.** Each task states what must already exist, checked before it runs. If
   thread 2 never wrote its report, thread 5 is not measuring memory.
9. **Seeding is a checked-in fixture, copied and never replayed.** A workspace
   tree and a SQL fixture go to both arms. A digest comparison then fails the
   run closed on any difference but the one preference row.
10. **The manipulation check gates every repeat.** In the lean arm the
    `Long-term Memory` and `Conversation` sections must be absent from round 2
    on, and present throughout in the control arm. `ContextCaptured` already
    carries the section list, so this is a read, not new instrumentation.
11. **Arms run interleaved per task, and repeats may run in parallel.** Arm A
    task `i`, then arm B task `i`. Provider drift then lands on both arms at the
    same point in the sequence. Repeats share no state, so parallelising them is
    safe. Parallelising the two arms of one repeat is not.
12. **Cost and quality land in one table.** Per thread the harness records
    `cache_creation_tokens`, cached reads, input and output tokens, and rounds.
    It also records the `TodoListWritten` count, recovery-tool calls after round
    1, wall clock, and a dollar figure from a checked-in price table.
13. **Three configurations, and the pilot sets the confirmatory size.** Smoke is
    1 repeat over 3 tasks and exists to debug the harness. Pilot is 3 repeats
    over 12 tasks and produces the variance estimate. The confirmatory repeat
    count comes from a pre-registered formula on the pilot's measured variance,
    capped at 25 by budget.
14. **The bar is pre-registered, the treatment is revisable, the bar is not.**
    The task set, the fact register, the probes and the bar are committed before
    the first confirmatory run. The harness stamps their hash on every result
    row. Revising the note-writing guidance and re-running is legitimate.
    Revising a probe after seeing a result invalidates that result.
15. **The eval is a binary, never a test.** It lives in a new
    `crates/lucidos-eval` and runs from `scripts/eval-context-mode.sh`. A
    `#[test]` would be picked up by `make test` and would spend four figures.

### The task set

Twelve threads. The rightmost column gives the reason it is in the set. In every
case that is a channel where losing context plausibly changes the answer.

| # | Thread | Exercises | Depends on | Why it is in the set |
|---|---|---|---|---|
| T01 | Ground the project | `write_file`, artifact taxonomy | none | Establishes two file-borne and three chat-only conventions. Measures nothing, by 0085's own constraint. |
| T02 | Seed and report | `emit_event`, `query_events`, `write_file` | T01 | Eight known events give arithmetic with one right answer. First scored thread. |
| T03 | Build the app | `create_app`, `refresh_app`, `capture_app` | T01, T02 | Apps are the workspace's most-built artifact. A stated style constraint is 0085's named lossy category. |
| T04 | The dead end | `run_bash`, `bash_output` | none | A tool fails for a discoverable reason, and the same capability is needed again later in the turn. 0085 calls this the most predictable regression. |
| T05 | The trigger | `create_trigger`, `load_knowhow`, `write_file` | T01, T02, T04 | A trigger carries a cron and an intent, and the taxonomy rule keeps procedure out of the intent. Three tier-3 probes in one artifact. |
| T06 | The knowhow round trip | `load_knowhow` | T04, T05 | The routing list stays in the prompt under 0085 and 0086. An internal validity control. |
| T07 | The sub-thread | `run_thread`, `follow_up_child_thread` | T02, T03 | State created outside the prompt, which 0085 names as a note category. |
| T08 | Background and wait | `run_bash_background`, `await_event` | T04, T06 | An event-wait delivery is a fresh round 1 under 0085's decision 13. Tests the re-entry rule directly. |
| T09 | The question | `ask_user_question` | T05 | The sixth busiest tool. Tests that an answer-driven resume returns the payload, and that an answered question is not re-asked. |
| T10 | The long run | everything | T01 to T09 | The nightly shape, 60 rounds and up. Best case for the saving and worst case for amnesia, in one thread. |
| T11 | Memory only | `memory`, `query_events` | T01, T02 | A fact held in no file and no event payload, reachable only through recall or a message scan. The pure memory channel. |
| T12 | The handover | `read_file`, `write_file` | all | The recall census, and the primary endpoint. A proportion out of roughly 20 facts, far lower variance than a binary. |

Everything runs locally. No task depends on the network, on a live external
service or on a coding agent. Each would add variance the design cannot absorb,
and coding-agent threads are out of 0085's scope by construction.

### The bar

**Graduate to the default when all four hold.**

1. **Cost.** Median cost per sequence run in the lean arm is at most 0.70 times
   the control arm.
2. **Quality.** The upper bound of the one-sided 90% confidence interval on the
   census difference, control minus lean, is below 0.10.
3. **No catastrophic task.** No single task's outcome-success rate falls by more
   than 25 points, and the two dead-end probes do not regress at all.
4. **Recovery is cheap.** Median rounds per task in the lean arm is at most 1.25
   times the control arm.

**Kill the flag when any one holds.**

1. Lean-arm cost is not below 0.85 times the control arm. The optimisation then
   failed on its own terms.
2. The census difference point estimate exceeds 0.15 in the control arm's
   favour.
3. Either dead-end probe regresses by more than 20 points.
4. `lost-silent` outcomes in the lean arm exceed 10% of scored probes.

**Anything else leaves 0085 experimental**, and the probe tier breakdown then
says what to change. That is the expected outcome, and it is a result.

### Which of 0085's open questions this answers

| 0085 open item | Answered by | Verdict |
|---|---|---|
| The floor: does the harness guarantee verbatim retention | T04 and T09, the two tier-4 probes, read against the tier-1 and tier-2 controls | **Yes.** Tier-4 regression with tier-1 and tier-2 flat means a within-turn tail is needed, and nothing else is. |
| Whether the notes stay visible | nothing | **No.** It is a question about a watching user, and the eval runs unattended. |
| Read-by-id on `query_events` | T02 and T11 record how often a pointer resolved through a newest-first window and missed | **Evidence, not a decision.** See the risk below: absent read-by-id, a lean-arm failure is not attributable. |
| A character cap on the notes | T10's output tokens per `todo_write`, against its census contribution | **Yes, as a curve.** It yields a cost per noted character, not a number to hardcode. |
| Which image handle replaces `thread:N` | nothing | **No.** `capture_app` appears in T03 and T10, but the choice between a stable id and an artifact path is a design call no outcome settles. |
| Within-turn `dismiss_from_context` | nothing | **No.** Out of scope, and no task exercises it. |
| The two largest fixed blocks | nothing | **No.** The tools array and the system prompt are constant across arms, so the eval is blind to both. ADR 0088 owns the first. |

## Rationale

**Task success is the wrong primary endpoint, and 0085's phrasing needs
sharpening rather than obeying.** A binary carries about one bit, and 12 of them
per run is a high-variance measure of a small effect. Worse, the failure this
flag risks is an answer that is delivered, fluent and quietly missing a
constraint. Scored on completion, that reads as a pass. The census scores the
detail directly. A proportion out of 20 carries perhaps a third the standard
deviation of a mean of 12 binaries.

**Planted probes beat a judge at the job a judge is hired for.** The obvious use
of a judge is "did the agent honour the constraint", and an assertion answers
that better, deterministically and for free. A judge on the same question adds
noise to a measurement already short of power, and it favours the longer answer.
Its honest job is triage, choosing which of 500 threads a human reads.

**The recovery tier is what turns a verdict into a fix.** 0085's claim is not
that nothing is lost, it is that everything lost has a way back. A tiered probe
set measures exactly that. A tier-1 loss says the agent stopped calling
`glob_files`, and a tier-3 loss says memory recall was load-bearing. One number
for the whole flag would leave you re-running the experiment to find out which.

**Void is not fail, and conflating them manufactures a regression.** The
sequence is deliberately dependent, so an early flake propagates. Without a
precondition check, one failed write in thread 2 becomes five failed probes
downstream. All of them get attributed to the arm rather than to the flake.

**Two workspaces, not one workspace with the flag flipped.** A crossover inside
one workspace looks cheaper and is not comparable. The moment the lean arm fails
to note something, the workspace state diverges. The later threads are then no
longer running against the same world. 0085 already specifies two, and this is
the mechanism behind that choice.

**Seed the memory index, despite the from-scratch constraint.** 0085 is right
that a fresh workspace has no recall. The control arm's advantage is therefore
zero for the first few threads. Seeding roughly 40 fixture entries with literal
vectors buys those threads back. The vectors are checked in, so they are
byte-identical rather than re-extracted, and extraction is an LLM call nobody
can reproduce.

**Interleave the arms, parallelise the repeats.** A provider-side change during
a run is the one confound with no defence except timing. Interleaving per task
puts both arms at the same point in the sequence when it lands. Repeats share no
state, so running three at once costs only host load.

**A binary rather than a test, and a crate rather than a script.** The scoring
needs the same Postgres and HTTP glue `lucidos-e2e` already carries, so a crate
reuses it. The distinction that matters is `cargo run` against `cargo test`. The
repo's rule sends every Rust change to the engine suite. An eval inside that
suite would spend a thousand dollars on a lint fix.

## Consequences

- **The eval can falsify "lean is fine" and cannot establish it.** Probes see
  only what somebody thought to plant. A 10% human sample of 500 threads will
  not reliably surface a subtle systematic difference. So graduation is
  conditional and reversible, and 0085's production re-read rate stays the
  standing monitor with a stated rollback condition.
- **The design is clustered, so the sequence run is the effective sample.** With
  12 tasks per run and an intra-cluster correlation of 0.1, the design effect is
  2.1, and 180 task outcomes behave like 86. The primary analysis is therefore a
  mixed model with a random intercept per run, or a cluster-robust test. A
  pooled two-proportion test would overstate the precision.
- **No affordable configuration resolves a small effect.** Fifteen repeats put
  the minimum detectable census difference near 0.15, at a standard deviation of
  0.15. Twenty-one repeats reach the 0.10 margin at the same variance. Five
  points is out of reach at any budget worth spending, and the bar is set where
  the instrument can see.
- **The pilot is not a result and must not be reported as one.** Three repeats
  exist to measure variance and to break the harness. Their difference has a
  confidence interval wider than any effect that would change the decision.
- **The bill and the clock are real.** A sequence run is roughly 350 rounds,
  which is about $20 at a 40,000-token prefix. A repeat is therefore near $45,
  the pilot near $150, and a 21-repeat confirmatory near $1,000. A round takes
  15 to 25 seconds, so a repeat runs about 4 hours. Running three at a time, the
  confirmatory needs 3 or 4 nights.
- **The eval measures the implementation, not the design.** A weak note-writing
  instruction sinks the lean arm and looks like 0085 failing. The guidance
  string is therefore a versioned fixture, and its hash is stamped on every
  result. A re-run with revised guidance is legitimate against the same bar.
- **Read-by-id has to land first, or a failure is uninterpretable.** 0085
  records that `query_events` has no id argument, so a noted pointer resolves as
  a newest-first window capped at 128 KB. Running the eval before that gap
  closes measures the design plus a known handicap. A fail would not say which
  one lost.
- **It cannot run in the same window as ADR 0086.** 0086 removes the file
  listing unconditionally and says so itself. The control arm has to be
  post-0086 behaviour, settled, or the comparison has two variables.
- **A new crate and a new script both carry paperwork.** The crate joins every
  contributor's `cargo build` and the lint surface. The script matches no rule
  until its path is added, and `CLAUDE.md` requires that in the same change.
- **The task set is a maintenance burden with a half-life.** It asserts on app
  manifests, trigger shapes and event payloads, so a change to any of those
  breaks the eval quietly. It is pinned to a commit, and the smoke configuration
  re-validates it before each run.
- **T11 is the fragile task.** Memory extraction is an LLM call, so whether the
  planted fact reaches the index is not reproducible. The harness asserts the
  fact is retrievable in both arms before T11 runs, and voids the task in that
  repeat when it is not.

## Alternatives considered

**An LLM judge as the primary scorer.** Rejected. It cannot see a planted probe
better than an assertion can, and it prefers the longer answer. Its noise adds
straight to the variance of a measurement already short of power. Kept for the
two fuzzy probes and for triage.

**A human read as the primary scorer.** Rejected on arithmetic. Five hundred
threads at three minutes each is 25 hours of reading, and a reader who knows the
arm is not a measuring instrument. Kept as a blinded stratified sample, which is
the only thing that sees an unplanted regression.

**One workspace with the flag flipped between threads.** Rejected. The
accumulated state is the independent variable, so a crossover contaminates the
second half with the first half's divergence.

**Replaying recorded conversations instead of running live agents.** Rejected.
The causal claim is about what an agent does when a section is missing, and a
replay cannot produce a different action. Kept as an optional fast path for
harness development, where the point is reaching thread 10 cheaply.

**Scoring cost alone and skipping quality.** Rejected. 0085 already measures
cost from production events and predicts the saving. The eval exists for the
half production cannot answer.

**A single long thread instead of a sequence.** Rejected. It would exercise the
conversation-history drop and say nothing about memory recall, which is the
other half of what 0085 removes.

**More tasks, fewer repeats.** Rejected. Under clustering the run is the
effective unit, so repeats buy power and tasks buy coverage. Past roughly twelve
tasks the marginal task adds a capability and almost no statistical power.

**Running the eval as a `cargo test` in `lucidos-e2e`.** Rejected. `make test`
would run it, and the repo's own test-selection rule sends every Rust change
there.

**Scoring each task all-or-nothing.** Rejected, and this is the difference
between three failure modes and one. Three failures need three columns: a
partially correct answer, a correct answer at twice the rounds, and an answer
missing one detail. Only a per-probe score, a rounds column and the
`lost-silent` count separate them.

## Deliberately left open

- **A third arm: sections dropped, notes tool absent.** The single most
  informative addition if budget allows. If the lean arm matches the control,
  and a no-notes arm also matches, the sections were never load-bearing. The
  `todo_write` change would then be dead weight, a large simplification for 50%
  more spend.
- **The exact prompt wording of each task.** It belongs in the fixture and will
  iterate against the smoke configuration. The plan carries a first draft.
- **The judge's model and rubric text.** It must not be the model under test,
  and beyond that nothing here decides it.
- **How the price table is kept current.** It is checked in so results are
  reproducible, which means it goes stale by design.
- **Whether the harness survives the decision.** A standing regression eval for
  context changes is an obvious second life, and nothing here commits to it.
