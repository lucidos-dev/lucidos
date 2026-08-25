# 0087: The context-mode eval is a paired two-workspace sequence scored on planted probes, not on whether the task got done

- **Status**: Superseded by
  [ADR 0110](0110-context-handling-benchmark.md)
- **Date**: 2026-08-18

The machinery survives and the bar does not. ADR 0110 keeps the seeded
workspaces, the fourteen tasks, the planted probes and the price table. It
retires the graduation and kill conditions, the route vocabulary, the
recovery-signature table and the census as a primary endpoint. Decision 14
freezes the bar at the first confirmatory run, and none happened, so nothing
pre-registered is broken. Read 0110 for what the eval scores today.

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
10. **The manipulation check gates every repeat.** The lean arm must carry a
    `Context Ledger` section on every round, and the control arm must never
    carry one. `ContextCaptured` already carries the section list, so this is a
    read, not new instrumentation. See the second amendment below for why the
    gate reads the ledger rather than an absent section.
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

## Amendment, 2026-08-20: a pair the arms disagreed on measures the classifier

Everything above assumes the two arms of a task meet the same engine, less the
flag. They do not. Before retrieving memory the engine asks a classifier whether
the turn needs any, and that classifier is an LLM call
(`memory/extractor.rs::classify_query`). When it answers no, retrieval is
skipped and the thread carries no memory payload at all.

The first complete run caught the arms answering differently on one task.
Control skipped retrieval on T01 and lean carried 25 results. That pair compares
a thread holding a memory payload against one without, which is the classifier's
doing and not the flag's. It also biases against lean, which carried the payload
on round 1 and dropped it from round 2.

This is the shape the T11 consequence above already names. Memory extraction is
not reproducible, so the harness voids T11 when the planted fact is unreachable
in an arm. Retrieval is not reproducible either, and it applies to every task.

### The rule

**Retrieval in one arm and not the other, on one task of one repeat, voids that
task's probes in both arms for that repeat.** The signal is the `MemoryRecalled`
event, which the engine emits only when the classifier answered yes.

**A pair where both arms skipped retrieval is kept.** The flag genuinely saves
less on such a turn. Voiding those would flatter the lean arm.

Two alternatives were rejected. Forcing `needs_memory` on measures an engine
nobody runs. Recording the disagreement without acting on it leaves the noise in
the numbers.

### What it changes above

**Decision 4's `void` gains a second cause.** It reads as an upstream
precondition failing. It now also means the arms did not meet the same engine.
Void is unchanged in every other respect: never a pass, never a fail, and in no
rate.

**Decision 12 records one more thing per thread**, whether the engine retrieved
memory at all. The field is defaulted, so a results file written before it still
parses. Both arms then read as no retrieval, which is agreement, so re-analysing
an old run voids nothing.

**The bar is untouched.** No graduation or kill condition reads the void count,
and none is added. Decision 14 freezes the bar, and a run whose sample is mostly
voided is reported rather than judged differently.

**The effective sample shrinks, and the report now says by how much.** It prints
the attempted pairs, the two void counts and what is left. A run that voids most
of its pairs carries far less evidence than its task count suggests.

**Nothing in the engine changed and no knob was added.** The harness reads an
event the engine already emits.

### The first complete run is confounded

Run `20200d8a1ea540f49a82f7b1e4378569` predates this fix. Its cost ratio of 0.91
includes at least one pair the arms disagreed on, and that pair biased against
lean. Its rows carry no retrieval field either, so re-analysing them voids
nothing. Read that ratio as confounded, and never against the bar.

The fix lands before the first confirmatory run, which is what decision 13's
smoke and pilot configurations exist for. The same change after a confirmatory
run would invalidate it under decision 14.

## Amendment, 2026-08-20: the gate reads the ledger, because absence no longer proves anything

ADR 0085's amendment retires the round-2 drop. `Long-term Memory` and
`Conversation History` now stay until the model releases them, and a model that
releases nothing is a legitimate result rather than a broken flag. Decision 10's
gate would then fail every lean repeat, on the one outcome the eval most needs
to observe.

Decision 14 allows this: the treatment is revisable, the bar is not, and the
`guidance_hash` distinguishes the re-run. Nothing about the bar moves here.

### The rule

**In the lean arm every captured round must carry a `Context Ledger` section,
and in the control arm no round may carry one.** The ledger exists only under
the mode. It is rendered in block 0 of every round, and it does not depend on
the model doing anything. It is therefore the no-op tripwire the old gate was:
a flag that changed nothing produces no ledger.

One narrowing carries over from the first amendment. A lean turn that assembled
no body at all builds no ledger, and that is not the flag failing. The gate
skips a round whose exchange never built one, the same way it already skips a
section round 1 never built.

### The name fix that came with it

Decision 10 named the two gated sections `Long-term Memory` and `Conversation`.
The second was wrong and the engine never emitted it for that block.
`Conversation History` is the cross-turn history the flag reorders.
`Conversation` is the within-turn delta the agentic loop appends, which the
flag has never touched. The names are corrected above and in
`crates/lucidos-eval/src/manipulation.rs`, which is where they are read.

### What it does not change

The two section names stay in the harness, because the per-thread cost table
and the recall census both still read them. Only the gate stops asserting on
their absence. No graduation or kill condition reads the ledger, and none is
added.

## Amendment, 2026-08-20: the silent-loss kill reads what the lean arm lost and the control arm kept

Kill condition 4 fired on the wrong arm. It reads `lost-silent` as a share of
the lean arm's own scored probes, against 10%. Pilot run
`901db33b487047a697db3625d8c84021` came in at 19.3% in the lean arm and 31.6%
in the control arm, on the same 57 probes. The condition would have killed a
mode that lost strictly less than the baseline.

The cause is that an absolute rate cannot tell two things apart. A probe the
lean arm lost because the mode dropped a body is the failure ADR 0085 exists to
rule out. A probe the lean arm lost because no arm could reach the fact is a
fixture defect. In this run 12 of the 57 pairs were the second kind. T11 records
one of them as unreachable by memory search in either arm.

### The rule

**Kill condition 4 now reads paired lean-only silent loss, against 5% of the
probes both arms scored.** A pair counts when both arms scored the probe, the
control arm passed it, and the lean arm recorded `lost-silent`. A probe lost in
both arms counts against neither.

Three things stay as they were. Both arms' absolute rates are still computed and
still printed, because they say how hard the fixture is. `asked` and `lost-loud`
are still visible failures and still outside this count, per decision 4. A void
still scores nothing, so a pair the classifier disagreed on drops out of the
comparison without a second filter.

The report also prints the counts either way round: lean-only loss, control-only
loss, and lost in both. **The kill is one-directional on purpose.** A mode that
loses ten facts the control arm kept, while keeping ten the control arm lost,
has still dropped information the user had. Netting the two would hide that.

### Why 5%

The old bar was 10% of one arm's scored probes. Moving to a paired numerator
loosens the criterion twice over: the count drops and the denominator does not.
Halving the threshold keeps the criterion about as strict as it was written to
be. It also removes fixture contamination the criterion never meant to measure.

The number also has to survive a small sample. At this run's 57 pairs, 5% is
three probes and one probe is 1.75%. So a single unlucky probe cannot kill the
mode, and a pattern can.

### Decision 14 allows this

The bar is frozen at the first confirmatory run, and no confirmatory run has
happened. The run that exposed this is a pilot, which is what decision 13's
smoke and pilot configurations exist for. Re-analysing that pilot under the new
criterion is legitimate for the same reason.

## Amendment, 2026-08-20: cost has two bases, and each condition names the one it reads

Cost condition 1 appears twice in the bar, once to graduate at 0.70 and once to
kill at 0.85. Both read one number: median dollars per sequence run. That number
mixes two things, and pilot run `901db33b487047a697db3625d8c84021` shows how far
apart they can be.

Per run the lean arm cost 1.42 times the control arm. Three tasks supplied 94%
of the excess, and in each one the lean arm simply took more rounds: T06 at +13
rounds and +$14.11, T09 at +6 and +$7.02, T07 at +6 and +$6.90. Per round the
two arms were $1.084 against $1.102, a ratio of 1.017. Input tokens per round
were 58,061 against 60,616, a ratio of 1.044. Over the five tasks where both
arms used identical round counts the cost ratio was 1.055.

An agent is not deterministic, which the eval already knows: it is why decision
1 makes the run the unit and decision 13 sizes a repeat count from measured
variance. Round count is that variance. Charging it to the mode asks a question
about the agent and reports the answer as a property of the flag.

### The rule

**Both figures are computed and reported. Each condition names its basis.**

| Condition | Reads | Why |
|---|---|---|
| Graduation 1, at most 0.70 | cost per run | It is the bill the user pays. |
| Kill 1, not below 0.85 | cost per round | It asks whether the optimisation worked. |

A per-round figure is each run's own dollars over its own rounds, and then the
median of those. It is never the median cost over the median rounds, which is a
number no run had. Input tokens per round are computed the same way and
reported, never judged.

The two conditions can now disagree, and that is the point. A mode cheap per
round whose agent took more rounds is not killed, and does not graduate either:
it lands at experimental with the rounds ratio saying why. A mode whose per-run
win came from the agent taking fewer rounds is killed on its per-round overhead.
Graduation condition 4 already caps the rounds ratio at 1.25, so the two halves
cannot drift apart unnoticed.

**The report also prints the identical-round subset**, the tasks both arms ran in
the same number of rounds. It is a diagnostic and no condition reads it. The
subset is whatever the run produced rather than a random sample, so it cannot
carry a decision.

### Decision 14 allows this

Same reason as the amendment above. The bar is frozen at the first confirmatory
run, and none has happened. Both amendments land before it.

## Amendment, 2026-08-20: a completion probe per task, because retention is not delivery

Every scorer in the eval asks one question: did the agent still know a fact it
was told. The 40 probes ask it, and so does T12's census. Nothing asks whether
the agent did the job.

T06 is the case that exposed it. The prompt was "Collect example-org/web the
same way, and add it to the report". P06.1 checks `load_knowhow` was called,
P06.2 checks the dead-end script was not retried, and P06.3 checks a
`*-health.md` exists and no `report*.md` does. None of them reads whether web
was collected, or whether the report mentions it. An arm writing an empty but
correctly named file scores like one that did the work.

That is not a gap in one task. Decision 2 makes the census primary precisely
because a task can be done while quietly losing a detail, and nothing was
checking the other half: that it was done at all. Every cost figure divides by
tasks nobody verified.

### The rule

**Each task carries exactly one completion probe, in `completion.toml`,
asserting that its deliverable exists and is right.** The loader refuses a task
with none and a task with two, because the completion rate's denominator is the
task count.

**A completion probe is not a probe.** It carries no fact, no *recovery tier*
and no recovery route, and its outcome vocabulary is its own: pass, fail, void.
It enters no retention rate, no tier breakdown and no silent-loss numerator.
Decision 4's seven values are untouched. Those measure the mode, and this
measures the agent; mixing them makes both unreadable.

**It replaces `wrong_default` with `deliverable`.** Decision 5 requires a
tempting wrong answer, because a probe whose only failure is producing nothing
measures loudness. A completion probe has no such requirement, since producing
nothing IS the failure it looks for. What it owes a reader instead is one line
naming the deliverable in the task's own terms.

**It is scored the moment its own task ends**, against the tree that task left,
and never with the retention probes after the run. Every later task rewrites the
workspace. P02.5 is the proof: it asserts eight `BuildObserved` events, T10
emitted eleven more, and the pilot records it as `lost-silent` in both arms. It
was defeated by scoring order rather than by the mode.

**Its scorer is an assertion, never the judge.** The judge runs after the whole
run and a completion probe is scored during it, so the two cannot both hold. All
twelve deliverables turned out to be expressible. The loader refuses a judged
one with that reasoning, rather than accepting a key it cannot honour.

### Divergence

**A task one arm delivered and the other did not voids that pair for cost and
for the retention probes.** Same rule as a *classifier disagreement*, through
the same machinery. The arms did different amounts of work, so the task's
dollars and its probes compare two different jobs.

**The completion outcomes themselves survive that void, and this is
load-bearing.** Suppose they were voided too. The arms would then agree on every surviving task
by construction, both completion rates would be identical, and the condition
below could never fire. A divergent pair is the completion signal and
the cost confound at once.

**A classifier disagreement does void completion.** The arms met different
engines on that task, so the delivery comparison is no more about the mode than
the retention one is.

### The condition

**Kill the flag when the control arm's mean completion rate exceeds the lean
arm's by more than 5 points.** This is a fifth kill condition, and it stands
whatever the cost figures say: a mode that finishes fewer jobs is not cheaper.

The unit is the sequence run, per decision 1, so the rate is a per-run
proportion averaged over runs. Pooling would read twelve correlated tasks as
twelve samples.

Five points is tighter than the census kill's 15, because the failures differ in
kind. A lost fact is a degraded answer. An undone task is no answer, and it is
what every cost figure is divided by.

It is loose enough for an agent that is not deterministic. Twelve tasks per run
make five points 0.6 tasks. So a lean arm consistently leaving one task undone
is over the bar, and one unlucky divergence is not. At a pilot's single repeat
any divergence clears it, which is one more reason a pilot prints no verdict.

### Decision 14 allows this

The bar is frozen at the first confirmatory run, and none has happened. The
`fixture_hash` now covers `completion.toml` too, so a run scored with completion
probes is distinguishable from one scored without them.

## Amendment, 2026-08-20: a scope rule on every prompt, because the arm pays for invented work

Over-delivery is not self-punishing in an A/B. When an agent does more than it
was asked, the bill lands on the **arm** rather than on the agent. An
uncontrolled scope difference then reads as the mode being expensive.

T06 of the first pilot is the worked example. The lean arm did the task, then
asked whether the collected project should also appear in the app. The driver's
unscripted fallback replied "use your judgment", so it reshaped the payload,
edited the app three times, captured it and rewrote a knowhow doc. That is 18
rounds against the control arm's 5, and $19.19 against $5.08.

Neither arm was asked that question, so the task's cost comparison measures
nothing about the flag. Three tasks of that shape supplied 94% of the run's
per-run cost ratio.

### Two changes, both symmetric across the arms

**A scope rule is appended to every prompt.** It lives in `tasks.toml` as one
`scope_rule` key, applied where `{marker}` is substituted, never copied into the
twelve prompts. Twelve copies would let one drift and reintroduce the asymmetry
silently. It tells the agent to do what was asked, not to extend the work, and
to name anything else at the end of its reply.

Three things it deliberately does not do. It does not invite a question: "ask
me" turns scope creep into a round of `ask_user_question` plus a fallback reply,
which is the same cost with an extra hop. It does not forbid a tool, so
T07's sub-thread and T10's six jobs stay in scope because their requests name
them. It carries no fact, so it cannot answer a probe.

It is in `tasks.toml` rather than in code because `fixture_hash` covers that
file. A constant in Rust would let the measured treatment change while the
recorded hash stayed the same, which is what decision 14 exists to prevent.

**The unscripted fallback closes scope instead of opening it.** A question the
script did not anticipate is drift by definition, so the reply now sends the
agent back to the request. `unscripted_answers` still counts exactly as before,
so the signal that drift happened is unchanged.

The reply is sent as free text structurally, keyed on the unscripted flag rather
than on its wording. Option matching is substring-based in both directions, so a
one-word label hides inside any sentence: "No" sits inside "Mention".

### The two amendments depend on each other

A scope-closing fallback can stall a task that genuinely needed an answer.
Before the amendment above, that stall was invisible: the arm would produce
less, no probe would ask whether it had, and the arm would still score well.
With a completion probe the stall fails loudly on the deliverable, which is the
correct signal and the reason these land together.

### The existing results are a closed set

Every file under `eval/context-mode/results/` was produced under the old prompts
and the old fallback. Nothing run after this is comparable to them, and the next
run is a fresh baseline. They are kept as the record of what happened, and not
rescored.

### Still open: detecting scope divergence rather than discouraging it

`unscripted_answers` already records the signal, 1 for the lean arm and 0 for
the control arm on T06, and no condition reads it. A rule makes divergence rarer without
detecting it. So a guard voiding a pair whose arms answered a different number
of unscripted questions is still worth building. It is
deliberately not in this change.

## Amendment, 2026-08-21: an arm is a registered workspace, so you can read it

Reading how an arm solved the twelve tasks meant copying the workspace and the
database by hand. The arms were created outside the gateway's registry, so
nothing routed to them. Their databases were named `lucidos_eval_lean_1`, which
the gateway cannot derive from a slug.

The copy step should not exist. An arm is a Lucidos workspace: it has a data
tree, a database and threads. The only thing it lacked was a registry entry.

### The rule

Each arm is registered with the gateway when it is seeded. It appears in the
picker, `/eval-lean-1/` routes to it, and the harness boots its engine on the
port the registry holds. The gateway then adopts that engine on the first proxy
hit, rather than spawning a second one. So the arm is browsable DURING a run as
well as after it.

Registration goes through a new gateway primitive,
`POST /~/api/v1/control/workspaces/adopt`. It registers a directory that already
exists and starts nothing. The slug is the directory basename, the port comes
from the registry's allocator, and a re-adopt of the same path keeps both. An
entry pointing somewhere else is a conflict naming both paths, never a silent
repoint.

### Autostart is off, and that is the point

An arm is a measurement, not a service. The gateway must never spawn an arm
engine on its own boot, because the harness owns when an arm runs. A background
engine writing to a workspace the next seed clears would corrupt the run it is
about to start.

The endpoint and its first caller answer this differently, on purpose.

`adopt` treats an ABSENT `autostart` as false for a new entry and unchanged for
an existing one. The flag is a picker toggle, so it belongs to whoever set it,
exactly as the display name does. A re-registration is not a decision about it.

The harness does not take that offer. It sends `false` on every seed, so an arm
toggled on in the picker is turned back off. The seed clears the data tree and
force-drops the database, and it does that before anything releases a
gateway-started engine. Overriding a toggle is the smaller harm.

### The database is renamed to what the gateway derives

`lucidos_eval_lean_1` became `lucidos_eval-lean-1`. The gateway derives
`lucidos_<slug>` and a slug is `[a-z0-9-]`, so the hyphens are not a preference.
Every identifier use is now double-quoted, because Postgres reads an unquoted
hyphen as subtraction.

The eval crate must not depend on the gateway crate (ADR 0014 §1). So the
agreement is a literal on each side, with a test naming the other.

### One engine per arm, and one Postgres cluster

Two consequences follow, and both are load-bearing.

The harness and the gateway must not each run an engine for one arm. In one
direction that is free: the gateway probes the registered port before it spawns
and adopts what answers. That only holds while the harness boots on the
registered port, which is why the port lookup is not an optimization.

The other direction needs two things, because browsing an arm lazy-starts a
gateway-owned engine that nothing then stops. So the harness asks the gateway to
release the arm before it binds that port, best-effort like the registration.
And `boot_engine` watches its own child while it waits for health. A healthy
port does not prove the engine answering is the one it started. Without that
check the harness would drive an engine it does not own, and its `stop_engine`
would kill an already-dead child.

The harness's `LUCIDOS_EVAL_PG_BASE` and the gateway's own cluster have to be
the same Postgres. During a run it does not matter, because the gateway adopts
the harness's engine and reads whatever that engine was pointed at. Afterwards
the gateway lazy-starts its own engine against ITS cluster's
`lucidos_eval-lean-1`. Pointed elsewhere, a browsed arm opens empty. This is
documented in the script header rather than enforced: the harness cannot see the
gateway's Postgres configuration, and guessing at it would be worse than saying
so.

### Registration never fails a run

No gateway, no local token, a conflict or a refused connection: each logs one
line. The run then carries on with the free-port behaviour it had before. The
eval spends real money over hours, and whether a dev gateway happens to be up is
nothing to do with the measurement.

### I5 is untouched

The `eval-` prefix guard, the symlink refusal and `checked_eval_workspace` are
unchanged. `LUCIDOS_EVAL_ROOT` now defaults to `~/workspaces`, so an arm is an
ordinary workspace beside every other one and the picker lists it without a
nested root of its own. Adoption takes an absolute directory, so the root is
free to change: the prefix guard, not the parent, is what keeps the harness off
a real workspace. Changed 2026-08-21, after a run left two arms buried one
level down.

## Amendment, 2026-08-21: the inverted default is a treatment change, and the gate survives it

ADR 0085's second amendment inverts the mode. An assembled body now leaves at
the end of the round it arrived on, unless the model calls `keep_in_context`.
Decision 14 allows this: the treatment is revisable, the bar is not, and no
confirmatory run has happened.

The `guidance_hash` distinguishes the re-run, because version 3 of the note
guidance ships with the inversion. The inversion itself changes no task, probe
or fact. The `fixture_hash` still moves, because the T11 fix below ships in the
same batch.

### Decision 10 was re-checked and needs no change

The gate looks like it should break, which is why this is written down. The
sweep drops bodies and the gate reads a section list. A mode that drops
everything sounds like a mode with no sections left to report.

It holds for three reasons, each independent of what the model does:

- The ledger is rendered in **block 0**, and the sweep only reaches block 1.
  Block 0 is byte-stable within a turn by construction, which is what makes the
  cache seam pay.
- The ledger is built from `curated_bodies` AFTER the drop, so a dropped body
  keeps its row. That is the whole point of a *handle*: it is the way back.
- The ledger does not depend on the model doing anything. A lean round that
  assembled a body carries one whether or not the model ever called a tool.

So the tripwire is still a tripwire. A flag that changed nothing produces no
ledger, in either direction, and the narrowing for a round that assembled no
body carries over unchanged.

### Decision 12 records one more thing per thread

The `ContextKept` count joins the per-thread row, beside the `ContextDismissed`
count already there. Together they say what the model held and what it let go.
The field is defaulted, so a results file written before it still parses and
reads as zero keeps.

The control arm is reported as measured-and-empty rather than omitted. A blank
cell reads as "not measured", and a mode-off workspace genuinely emits neither
event.

### `repeat_recoveries` changes meaning without changing code

It counted the same handle recovered twice. Under the old default it was a
tripwire on a mode releasing nothing, and it read 0 on all 22 pilot threads.
Under the inverted default it is the primary cost: a body the model needed and
did not keep costs a round to read back.

No condition in the bar reads it, and none is added. It is a diagnostic that now
points at the guidance rather than at the engine.

## Amendment, 2026-08-21: T11 is scorable, and a probed fact has to be said

Not part of the inversion. This fixes a defect the pilot exposed, and it would
have distorted a re-run whichever default was in force.

### What was wrong

The pilot voided T11 in every repeat. Its `memory_precondition` asked for F05 to
be reachable by memory search in both arms before the prompt went out. Memory
extraction is an LLM call, so the gate fired whenever either extractor flaked.
It voided the task in both arms, which is exactly when the task is interesting.

Behind it sat the real defect. F05 is probed by P02.4, P10.5 and P11.1, and no
prompt ever stated it. T01 asked for a conventions file and named two
conventions, and the decimal rule was not one of them. A probe on a fact nobody
said measures nothing: the model cannot lose what it was never told. The
precondition then hid the hole, so the pilot reported a void rather than a
defect.

### The fix

T01's prompt now states F05, beside three other things the user says in passing
and does not want written down. The precondition is retired, along with the
`MemoryPrecondition` type and the reachability probe behind it.

A loader check replaces it. Every probed fact must be shown to have been said,
and the fixture is refused at load otherwise. Each fact declares the route:

| `established_by` | Where the check looks | Facts |
|---|---|---|
| `prompt` (default) | the establishing task's prompt | the other sixteen |
| `answer` | that task's scripted reply | F18 |
| `work` | nowhere, and the register says so | F12, F13, F14 |

`work` is not an escape hatch, it is the honest entry. F12 is discovered by
running a script that fails. F13 is an event the thread emits. F14 is a knowhow
file the thread writes. Nobody states any of them, and a check pretending
otherwise would be the same lie in the other direction.

Decision 5 asks you to name the tempting wrong answer, or the probe is not one.
This is its mechanical sibling: name the place the fact was said, or the probe
is not one.

### What it costs and what it retires

The "T11 is the fragile task" consequence above is retired, and so is the void
it describes. Extractor noise is now symmetric and lands in the interval, where
voiding deleted the datum. That is the trade: a wider interval on two probes,
against a task that produced no data at all in twelve threads.

The retrieval-symmetry rule is untouched. It reads `MemoryRecalled` per task per
repeat, it is a different mechanism, and it still applies to every task.

Seeding F05 into the memory index directly was the alternative, and it is worse.
It would make the arm's index part of the harness's setup rather than part of
what is measured. T11 asks what survives once the payload is gone, so the fact
has to arrive the way every other fact does.

The `fixture_hash` moves, because `tasks.toml` and `facts.toml` both change. The
pilot and any run against this fixture are not comparable on the T11 row.

## Amendment, 2026-08-21: the bar scores headroom, because cost was never the axis

Every run so far has stayed at roughly half the context budget. On
`claude-opus-5@default` the engine reserves 8,000 tokens for the response and
converts the rest at 1.5 chars per token. That leaves 288,000 chars for the
system prompt, the tools array and the messages. The largest messages block ever
observed is 145,309 bytes. So `trim_context_if_needed` has never fired, in
either arm, in any run.

That matters, because the trim path is the thing 0085 replaces. Every conclusion
the eval has produced describes a thread that never filled up.

### The axis the mode competes on

Control's answer to a full context is a blind eviction from the oldest end. The
model is not told, the drop leaves no trace, and there is no way back. Lean's
answer is a release the model chose, recorded in a ledger it can read and
re-fetch from. The comparison is what survives a full thread, not what a round
costs.

Cost condition 1 pointed the bar at an optimisation, and the mode is not one.
Read as written, a cost-neutral result with quality held is a kill, whatever the
mode did at the ceiling. That is the wrong question, asked precisely.

### Two trim paths, and they fail differently

| Path | Where | What it does | Task |
|---|---|---|---|
| In-turn | `context.rs::trim_context_if_needed` | Pass 1 replaces every oversized tool-result body with a stub naming its size. The `ToolUse` block survives, so the loss is loud. Pass 2 then evicts the oldest message pairs. | T13 |
| Cross-turn | `chat/process/run.rs`, at turn setup | Drops memory context, then history from the oldest end. No stub, no event, no notice. Turns simply disappear. | T14 |

Only the first writes something an event can see, and `ContextCaptured.trimmed`
carries it. The second runs before any capture exists and emits nothing, so a
run can cross it and leave no record at all.

**Amendment.** The in-turn row describes the trimmer as it stood for this run.
[ADR 0103](0103-context-trim-passes-and-the-persist-on-demand-verdict.md)
renumbered it to six passes and stopped pass 1 cutting every oversized body
whatever the shortfall. Eviction is now pass 5, and it is reached only after
four stubbing passes run out of material.

### The task set above is now fourteen threads

| # | Thread | Exercises | Depends on | Why it is in the set |
|---|---|---|---|---|
| T13 | The five documents | `read_file`, `write_file`, `emit_event` | none | 193 KB of seeded material, read early, then housekeeping that never reopens it, then a deliverable needing one number from the first document. The in-turn ceiling. |
| T14 | The risk register | `read_file`, `write_file`, a follow-up prompt | none | The same corpus, then a second user turn asking for a fact that lives in one document. The cross-turn boundary. |

Neither declares a precondition, so both run standalone. Their three facts are
F21 to F23, and each is written in exactly one document and nowhere else. A task
carries an optional `followup` prompt for the second turn, posted only once turn
one is genuinely finished and holding no live event wait. One deadline covers
both turns, and a first-turn timeout suppresses the follow-up entirely.

**The census is scoped, and not by hand.** T12's recall census now covers the
facts established upstream of T12, plus its own. F21 to F23 are established in
T13 and T14, which T12 does not depend on, so the handover has no route to them.
Unscoped they would become three census probes neither arm could pass, scaling
the primary endpoint by 20/23. A per-fact opt-out was rejected: a flag on a
pre-registered endpoint is a lever somebody can pull after seeing the result.

The completion margin keeps its 5 points, and the arithmetic under it moves.
Five points of fourteen tasks is 0.7 rather than 0.6, so a lean arm consistently
leaving one task undone is still over the margin.

### Cost becomes a bound

| Condition | Was | Now | Basis |
|---|---|---|---|
| Graduation 1 | at most 0.70 | at most 1.10, and at most 1.25 per run | both |
| Kill 1 | not below 0.85 | above 1.25 | per round |

Cost stays bounded and stops deciding. A mode nobody can afford is still killed.
A mode costing about what control costs is no longer killed for failing to save.
Graduation 4's cap on the rounds ratio at 1.25 is untouched, so a run cannot buy
a cheap round by taking more of them.

Nothing else moves. Graduation 2 and 3 are unchanged, and so are kills 2 to 5.
The census margin, the dead-end margin and the silent-loss margin all keep their
numbers. **The bar is looser on cost and no looser on quality.** That is the only
direction this amendment may go, and the two conditions below are the price.

### Graduation condition 5: headroom

Both halves must hold.

- **The run crossed the ceiling.** At least one control-arm T13 thread recorded
  a trimmed round.
- **At the ceiling the mode held.** Over T13 and T14, lean-only silent loss does
  not exceed control-only silent loss, and completion does not fall by more than
  5 points.

A run that never crossed measured nothing here, so it lands short of Graduate
rather than being killed. That is the shape the dead-end conditions already use.
An unmeasured bar cannot be cleared, and failing to measure it is not a failure
to clear it.

The first half reads T13 alone, because T14's crossing leaves no event behind. It
reads a control thread and not a lean one, because the control arm is the one
whose context the material has to overflow. A lean arm that curated well may
never trim, and that is the result rather than the setup failing.

### Kill condition 6: the ceiling

Lean-only silent loss over T13 and T14 exceeds control-only silent loss by more
than 10 points of the pairs both arms scored.

Read against the control arm's own silent loss, and never against zero. Blind
eviction loses things too, so the question at the ceiling is which arm loses
more. A mode whose chosen release loses more than an oldest-first eviction has no
reason to exist. That holds whatever it does on the threads that never fill up.

### What the harness records

Each thread row gains a `trimmed_rounds` count, in decision 12's shape:
defaulted, so an older results file still parses and reads zero. Zero reads as
"this thread never overflowed", which is true of every run so far.

The ceiling task set is a constant in `analyse.rs`, beside `CENSUS_TASK`, and
never a key in `tasks.toml`. A fixture key would be a tunable lever on a frozen
bar. A mode failing the headroom condition could then be rescued by moving a task
out of the set.

### The measured baseline

Run `987575b3ffd141dcacda34a5c52d9e34`, 2026-08-21, smoke configuration, one
repeat, `claude-opus-5@default`. Six task pairs ran and one was voided on
classifier disagreement. Over the five scored pairs, cost per round was 0.98 and
input tokens per round 0.998. Cost per run was 1.39, at 1.375 times the rounds.

Quality held. Lean-only paired silent loss was 0. Absolute silent loss was 22.2%
against the control arm's 27.8%, on 18 scored probes each. Three tier-3 probes
were answered from notes rather than from a body.

Those figures come from one workspace, on Anthropic only, with one model at
default effort. It was a smoke run of one repeat, so decision 10 withholds a
verdict and the numbers are not reproducible from the repo. Above all it measures
threads that never filled up, which is why this amendment exists.

### The honest reading

**That run does not graduate under the amended bar.** It fails three conditions.

Graduation 1 fails on its per-run half: cost per run was 1.39 against a cap of
1.25. The per-round half passes, at 0.98. Graduation 4 fails at 1.375 times the
rounds, against a cap of 1.25. Graduation 5 fails because the run never crossed
the ceiling, so the headroom condition measured nothing.

It is not killed either. Cost per round at 0.98 sits far inside the 1.25 kill,
and no quality kill fires. It lands at experimental, which is what the bar is
for.

So the cost change is not a rescue, and nothing here is written to let the
measured run through. 0085 cannot graduate until a run crosses the ceiling and
T13 shows a difference there. If a run crosses and shows none, the mode is a
cost-neutral no-op, and the bar will say so.

### Decision 14 allows this

Same reason as the amendments above. The bar is frozen at the first confirmatory
run, and none has happened. This lands before it.

The `fixture_hash` moves, because `tasks.toml`, `probes.toml`, `facts.toml`,
`completion.toml` and the seed tree all change. No earlier run is comparable to
one scored against this fixture.

## Amendment, 2026-08-22: the harness pins the classifier, and the void stays as a net

The first amendment on this page voided a pair whose arms disagreed on
retrieval, and rejected two alternatives. One of them was forcing
`needs_memory` on, because that measures an engine nobody runs.

The void works, and it is expensive. In the run of 2026-08-21 it took T13 and
T01, the two most valuable tasks in the fixture. T13 is the in-turn ceiling
task. It is the only place a run can show it crossed the context budget, so the
whole ceiling result rested on T14 alone. Graduation 5 then failed for want of
evidence rather than for want of headroom. A safety net that removes the
measurement is not free, and voiding is not the cheapest way to buy
determinism.

### The rule

**Both arms boot with `LUCIDOS_FORCE_QUERY_CLASSIFICATION=all`.** The engine
reads that variable once per process. Set to `all` it uses
`QueryClassification::default()` and never makes the call. Set to `none` it
needs nothing. Unset, or set to anything else, it classifies exactly as it
always did.

**The pin is `all`, deliberately.** Long-term memory is one of the three
sections the curated context mode can drop (`MEMORY_SECTION` in
`context_mode.rs`). Retrieving it on every turn is what gives the mode
something to drop. `none` would hide the mechanism under test.

**The void stays.** `analyse::classifier_voided_pairs` and
`retrieval_disagreed` are untouched. They should now never fire, and a run
where they do has lost its pin.

### What it changes above

**The rejected alternative is narrowed, not reversed.** "Forcing `needs_memory`
on measures an engine nobody runs" still holds of the production default, and
the default is unchanged: an engine with the variable unset behaves as it did
before. What is rejected now is only the wider version, pinning the classifier
for everybody. A harness that pins its own two engines is measuring the flag,
which is what it was built to do.

**Decision 4's `void` keeps both causes.** Nothing about the void changes. The
second cause simply stops arising.

### Decision 14 allows this

Same reason as the amendments above. The bar is frozen at the first
confirmatory run, and none has happened. This lands before it, and it moves no
threshold: every graduation and kill condition reads exactly as it did.

The `fixture_hash` does not move. The fixture is untouched, and what changed is
how the two arms are launched.

## Amendment, 2026-08-22: a run that measured no cost cannot clear a cost bar

Three conditions read a dollar ratio. Graduation 1 reads both bases, and kill 1
reads the per-round one. Each ratio is the lean arm's figure over the control
arm's, and `analyse::ratio` returns 0.000 when the denominator is zero.

So a model priced at zero produces 0.000 on every cost ratio, and all three
conditions pass. The run reads as the cheapest result the bar can express. In
fact nothing was weighed.

`stealth/ox-alpha` is the row that exposed it. It is a free stealth model, so
all four of its per-mtok prices are 0.0 and the price row is correct. What was
wrong is that the gates could not tell "free" from "cheap".

### The rule

**Cost is measured only when the control arm's median run cost is above zero.**
The control arm is the denominator of every dollar ratio, so a control arm
billed nothing leaves each of them zero over zero.

**An unmeasured cost fails graduation and kills nothing.** The failure states
what happened: the control arm cost $0.00, which is what a zero-priced model
produces, so neither ceiling could be read.
Kill 1 stays silent, because a kill reads "the mode is too expensive" and that
is the opposite of what such a run shows. The intended reading is that cost is
unmeasurable here, so the run is judged on retention, rounds and completion.

This is the shape the dead-end conditions and graduation 5 already use. A bar
nobody measured cannot be cleared, and failing to measure it is not a failure to
clear it.

**The type carries it, not the gate.** `CostResult` gains a `cost_measured`
flag, so a reader of the struct cannot take a ratio for a result. Two
diagnostics go with it: the identical-round subset's cost ratio and the paired
bootstrap interval are unreadable the same way.

**The report says so on the line.** The two cost lines print an explicit "not
measured" instead of `$0.00 ... ratio 0.000`. Their "graduation reads this" and
"the kill reads this" annotations go with them, because no gate read them.

### What it does not change

Only the dollar-denominated conditions move. Graduation 4's rounds ratio and the
reported input tokens per round are priced by nothing, so both stay live in an
unpriced run. The census, the silent-loss counts, the ceiling result and
completion are all untouched.

No threshold moves either. The three cost conditions read exactly the numbers
they read before, on every run that was billed something.

### Decision 14 allows this

Same reason as the amendments above. The bar is frozen at the first confirmatory
run, and none has happened. This lands before it.

Neither the `fixture_hash` nor the `guidance_hash` moves. Pricing lands in
`prices_hash`, which the ox-alpha row already moved.

## Amendment, 2026-08-22: an unrecovered empty completion voids the task

The model can end a turn with no text and no tool call.
`classify_empty_completion` treats a clean stop as benign, which is right for
the product: the thread goes idle with no error and the UI shows a neutral note.

The harness read that as a real attempt. `TaskStatus::Idle` counts as
`completed()`, so a thread that did nothing was recorded as a task that ran and
failed to deliver.

Run `6adb6572ad04470583e528c28b42ad68` is what it costs. The lean arm's T06
thread was one round, zero tokens, zero wall clock, and no work at all. C06
therefore failed in lean, the pair diverged, and T07, T09, T10 and T12 voided
downstream. All 45 retention probes voided, both ceiling tasks went unscored,
and the run reported a completion kill of lean 70.0% against control 92.9%. Not
one number in it was about context mode.

An unretried empty completion does not degrade a run. It deletes it.

### The rule

**A turn that produced nothing is re-posted, at most twice.** The predicate is
two halves, and both are needed. No `ResponseGenerated` in the turn carried
text, and the turn made no tool call. Input tokens corroborate it and are
logged, and never decide it: a provider that billed the round and returned
nothing produced the same non-attempt.

Each half rules out a false positive the same run supplied. A terse answer has
text. A turn of tool work that closes with an empty message has tool calls. Two
of that run's three empty completions were exactly that second shape.

**An empty completion that survives the retries voids the task in both arms.** A
thread that never ran did not fail to deliver, and the arm that did deliver has
nothing left to be compared against. The void reason names the empty completion,
so a reader can tell it from a classifier disagreement and from a scope
divergence. It is attributed first of the three, because a thread that never ran
cannot meaningfully have disagreed about anything.

**Reported, never judged.** The report states the attempts that came back empty,
the re-posts spent, and the threads that recovered and did not. An unrecovered
one prints a line saying the completion figures above cover the tasks that ran.
No graduation or kill condition reads any of it.

### What it changes above

`TaskStatus::Empty` is a new terminal status, written as `empty-completion`. It
is not `completed()`. So the task's own probes void, its downstream tasks void
in that arm, and no follow-up is posted into a turn that never happened.

The thread row gains `empty_completions`, `empty_retries` and
`followup_sequence`. The last is the load-bearing one: a re-post writes its own
`MessageReceived`, so counting prompts no longer finds the turn-two boundary. The
driver records the boundary it drove, and the scorer reads that. A run recorded
before the field falls back to counting, which is what those runs meant.

### What it does not change

The engine is untouched. A clean empty stop stays benign, which is correct for
every thread that is not an eval task.

No threshold moves. The census, the silent-loss counts, the ceiling result and
the cost figures read exactly what they read before, over the pairs that
survive.

### The engine already covers one cause of it

All three empty completions in that run were `stealth/ox-alpha` over OpenRouter,
inside three minutes. Each ended with no content, no tool call and no usage
block at all. The OpenAI-compatible provider now refuses that shape as
`OpenAI stream truncated`, which `is_transient_error` matches, so the request is
re-issued below the harness. That guard landed four minutes before the engine
binary under test was cut, and missed it.

The harness retry is still second-line defence rather than redundant. The guard
fires only when there is no usage at all, so a provider that reports usage and
returns nothing reaches `classify_empty_completion` untouched. And the harness
must not score a thread that never ran, whatever produced it.

### Decision 14 allows this

Same reason as the amendments above. The bar is frozen at the first confirmatory
run, and none has happened. This lands before it, and it moves no condition:
the counts are reported and nothing reads them.

Neither the `fixture_hash`, the `guidance_hash` nor the `prices_hash` moves. No
fixture, guidance file or price row is touched.

## Amendment: every dollar figure above is about four times too high

The harness priced a thread by adding four token counts, each at its own rate.
Three of those counts overlap. `input_tokens` is everything the model processed,
and it already contains `cache_read_tokens` and `cache_creation_tokens`. So the
cached tokens were billed twice, once at the cache rate and once at the full
input rate.

The figures are left standing rather than rewritten. They are what this ADR
read at the time, and re-pricing history in place would hide that the reading
was wrong. Recomputed from the run 6 results file at the same pinned prices:

| | reported | corrected | overstated |
|---|---|---|---|
| control | $113.54 | $27.71 | 4.10x |
| lean | $120.61 | $30.68 | 3.93x |
| both arms | $234.15 | $58.39 | 4.01x |

**No verdict in this ADR moves.** Every condition here reads a ratio, and the
overstatement applies to both arms at once. Cost per run goes from 1.062 to
1.107 and cost per round from 1.043 to 1.087, both still under the 1.25 kill.
Cost per round stays under the 1.10 graduation bar, where it now sits close
instead of comfortably below.

What does move is the scale the mode has been argued at. The absolute gap
between the arms on run 6 is about $3, not $7. Read this as a correction to the
instrument, not as a reprieve for the mode.

ADR 0110 supersedes this document and carries the fix. `InputSplit` in
`crates/lucidos-eval/src/metrics.rs` is now the only way a price reaches a token
count, and it takes the cached parts out of the total first.

## Amendment, 2026-08-24: and the Anthropic price row was 3x too high on top of that

Two independent errors, so they multiply. The amendment above fixes the
arithmetic. This one fixes the prices the arithmetic ran on.

`eval/context-mode/prices.toml` priced `claude-opus-5@default` at $15.00 fresh
input, $75.00 output, $1.50 cache read and $18.75 cache creation per Mtok. Every
one of the four is exactly three times the real rate. Opus 5 is $5.00 input,
$25.00 output and $0.50 cache read, and Anthropic bills a 5-minute cache write
at 1.25x input, so $6.25. The row had never described Opus 5. It carried Opus
4.1's rates, which are the figures it listed.

This is not the file going stale, which its header allows for and answers with
"add a row and re-run". A price that moved leaves an old run correctly priced.
A row that was wrong from the start never priced anything correctly.

**Every dollar figure on this page priced at `claude-opus-5@default` is 3x
high, on top of whatever the amendment above says about it.** That is all of
them bar one: the `stealth/ox-alpha` figures are four zeros and are correct.
The other two Anthropic rows moved by different factors, 4.5x for `[1m]` and
1.5x for Sonnet 5, and nothing here was ever priced against either. Run 6
recomputed on both errors:

| | reported | double count removed | and repriced |
|---|---|---|---|
| control | $113.54 | $27.71 | $9.24 |
| lean | $120.61 | $30.68 | $10.23 |
| both arms | $234.15 | $58.39 | $19.46 |

The per-arm figures round to the cent independently, so they sum a cent above
the pair total.

The budget estimate near the top of this page moves the same way. A sequence run
is about $7 rather than $20, a repeat about $15, the pilot about $50, and a
21-repeat confirmatory about $333.

**Nothing downstream moves, and there is no bar left for it to move.** ADR 0110
retired the graduation and kill conditions outright. A human reads five absolute
axes now, and no threshold is cleared or missed. Even on the arithmetic this
page used, each ratio is untouched, because one factor hit all four line items:
cost per run 1.107 and cost per round 1.087 are what they were. What moves is
the scale, again. The absolute gap between the arms on run 6 is about $1, not $3.

The figures above this amendment stay as they were read. Re-pricing them in
place would hide that the reading was wrong, which is the same reason the
previous amendment gave.
