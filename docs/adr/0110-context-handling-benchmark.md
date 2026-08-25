# 0110: The context-mode eval is a single-configuration benchmark of context handling, scored on five absolute axes

- **Status**: Accepted
- **Date**: 2026-08-23

Supersedes [ADR 0087](0087-context-mode-eval-graduation-bar.md) and implements
the "what the next eval run needs" list in
[ADR 0109](0109-model-writes-notes-and-sees-its-own-context.md).
Its record is `docs/plans/2026-08-23-context-handling-benchmark.md`.

## Context

ADR 0087 built an instrument to answer one question: does ADR 0085's lean flag
beat its control arm. Its headline is a table of ratios and a graduation/kill
bar. Six runs later, three things have made that question unanswerable and
uninteresting.

**The mechanism it graded no longer exists.** ADR 0109 supersedes ADR 0085. The
ledger is a context panel and the model has a scratchpad. `keep_in_context` buys
one round instead of a thread. `dismiss_from_context` and the assembled body
region are gone. Kill 6 and graduation 5 read a design that shipped and was
replaced.

**The lever it graded governed 1.7% of the request.** Measured over 132 lean
captures in run 6: tool definitions 51,609 chars, system instructions 33,563,
the live message array 32,465, and the curated bodies the verbs could reach
2,144. On the 24 requests that crossed the budget the curated region was empty
and the message array was 137,752 chars. Zero keeps was close to the correct
play under those conditions.

**The route vocabulary is not comparable across arms, by construction.** A
control pass is `in-prompt` unconditionally, because nothing was ever dropped.
So a run showing 44 `unknown-pass` in lean against 0 in control has shown you the
arm definition and not a behaviour. Eleven probes were rewritten across runs 1 to
5, and every one of them had been scoring the fixture's phrasing.

What survives all of that is the machinery: a seeded workspace, fourteen tasks a
driver can run unattended, planted facts with tempting wrong answers,
deterministic scorers, a price table, and a results file. That machinery answers
a better question than the one it was built for.

## Decision

**The eval measures one configuration doing work, and reports what it did.** A
*configuration* is the pins under test: the model, its reasoning effort, the
context-mode flag, and the declared context window. Fourteen decisions follow.

1. **A run measures one configuration.** `--arms` defaults to `lean` alone. A
   run may still name two arms, which interleaves them as before, and the
   baseline stays runnable for reference. The paired path is the exception now,
   not the shape.

2. **Absolute numbers lead, and a ratio is an appendix.** The report opens with
   one block per configuration: what this run actually did. A comparison block
   prints only when a run carried two arms. It prints differences beside both
   absolute figures rather than a bare ratio.

3. **The graduation and kill bars are retired.** `Verdict`, the four graduation
   conditions, the six kill conditions, the Cochran-Mantel-Haenszel secondary,
   the confirmatory-size formula and the widened margin all go. ADR 0087
   decision 14 freezes the bar at the first confirmatory run. No confirmatory
   run ever happened, so retiring it breaks no pre-registration.

4. **Five axes, and they are the whole report.** Each is absolute and each would
   still mean something for a model released next year.

   | Axis | What it reads |
   |---|---|
   | Task quality | delivery, fidelity, handover, and the failure split |
   | Cost | dollars per run, task and round; fresh input, cache read and cache write, per round |
   | Rounds | rounds per task, and re-fetches after round 1 |
   | Wall time | seconds per task and per run |
   | Context utilisation | peak and mean request size, headroom at the peak, trims by pass |

   Task quality has four scorers, and 79 graded items per run.

   - **Delivery**, 14 items, one per task. The deliverable exists and is right.
   - **Fidelity**, 45 planted probes. A fact the task needed survived.
   - **Handover**, 20 probes. The final document states each established fact
     with its own value.
   - **The failure split** on fidelity: wrong and silent, asked the user, or
     said out loud it no longer had the fact. That is behaviour under loss, and
     the three are not equally bad.

5. **No mechanism appears in any criterion.** No probe, rubric, wrong-default
   line, deliverable line or report line may name `keep_in_context`,
   `dismiss_from_context`, the ledger, the panel, a curated body, or a route.
   `Fixture::validate` refuses a fixture that does, against one banned-token
   list. This deletes the four pass-route outcomes, the recovery-signature
   table, every fact's note regex, and the per-thread keep and release counts.

6. **A probe may be failed only by forgetting, never by wording.** ADR 0087
   decision 5 stands, and `wrong_default` stays required. The test to apply
   before adding one is unchanged. Ask what the agent would have to forget to
   fail it, and delete the probe if the answer is only "word it differently".

7. **Deterministic first, judge only where unavoidable.** 78 of the 79 graded
   items are assertions. Exactly one probe is judged, on three votes. The judge
   is never the model under test, and the loader still refuses a judged
   completion probe.

   **Scoring reads the event log and the arm's data tree, not the log alone.**
   Delivery and most of fidelity assert on files the task wrote; the rest assert
   on events and on the thread's own tool calls. Both are what the run produced,
   and the workspace is seeded to three tables so nothing else could have.

   **Cache read and cache creation are reported apart, per round.** A read is
   0.1x and a write is 1.25x, so one dollar figure hides which happened. A
   configuration that edits its message array in place re-creates the array
   every round and reads none of it back. That costs several times what
   extending it costs. The frozen read is the tell, and only the split shows
   it.

8. **Context utilisation is a first-class axis, and a smaller window used well
   is a win.** The run reports peak and mean request size. It reports headroom
   at the peak against the declared budget, and the peak as a percent of it. It
   reports the rounds that trimmed, and which trim passes fired.

9. **The budget sweep is the pressure knob, and it is a seeded row.** The engine
   derives its char budget from `models.context_window`, and the fixture seeds
   exactly one model row. So a sweep declares a smaller window per run and
   changes no engine code. A sweep is several invocations, pooled by
   `analyse --run-id` over all of them.

10. **The smallest budget at which quality holds is the headline, and its rule
    is fixed here.** Quality holds at window `W` when two things are within 5
    points of their value at the largest window in the sweep: delivery at `W`,
    and fidelity at `W`. Five points is 0.7 of the 14 tasks, and 3.3 of the 65
    fidelity items. So one unlucky probe cannot move the answer and a pattern
    can.

11. **A window whose message budget collapses is refused, not measured.** The
    tools array and the system prompt are roughly 85,000 chars, and the budget
    is `(window - 8000) * 1.5`. Below about 64,000 tokens the message budget is
    zero, so every round ships over budget and the run measures the overhead
    rather than the agent. The harness refuses a declared window under 72,000
    and states that arithmetic.

12. **Every run captures its own requests in full, in its own event log.** The
    arm records whole `ContextCaptured` bodies, so the replay source is the
    workspace rather than a second file format. Two things make that true, and
    both were missing. The fixture seeds `capture_context`, without which every
    section body is `None`. And the arm's engine boots with
    `LUCIDOS_EVAL_FULL_CAPTURE`, which lifts the two 8,000-char body caps.

    **The gate changes `content` and never a section's sizes.** Both are the
    assembled length in both paths already, so the budget bar and the
    utilisation axis stay honest whatever the body does.

    **`trim_passes` joins the capture.** `trimmed` says a round lost content and
    not where from. Only pass 5 removes a message; every pass above it leaves an
    addressed stub. So the mask is what says whether anything went silently.

13. **The validity gate stays, and is never a criterion.** A lean round must
    carry a context panel and a control round must not. That is the tripwire
    against a flag that changed nothing. It aborts a repeat rather than scoring
    one, and no number it produces reaches the report. Decision 5 bans mechanism
    from the criteria, and this is not one.

14. **A price reaches a token count through one type, `InputSplit`.** The
    stored `input_tokens` is everything the model processed, and it CONTAINS
    both cache counts. Both providers are normalised to that convention:
    `anthropic_wire` sums Anthropic's three disjoint counts, and OpenAI's
    `prompt_tokens` already covers its cached tokens. So the harness takes the
    cached parts out of the total once, in one place. The resulting type has
    private fields and one constructor, so no path runs from a stored row to a
    dollar figure without it.

15. **The model is an axis of the run's NAME, and never of the arm.** An arm is
    a context-mode configuration and stays one. But an arm workspace was called
    `eval-<arm>-<repeat>` and its database `lucidos_<slug>`. So two runs against
    different providers both wanted `eval-lean-1`, and both tried to create
    `lucidos_eval-lean-1`. The second corrupts or fails against the first, and
    running providers in parallel is the point.

    **A run label leads the name: `eval-<label>-<arm>-<repeat>`.** Leading, so
    one run's workspaces sort together in the picker and in `ls`. The arm and
    the repeat stay where the eye already reads them. `eval-` stays the leading
    prefix, because invariant I5 refuses any directory without it.

    **The label defaults to the model id and `LUCIDOS_EVAL_RUN_LABEL` overrides
    it.** The default is what makes a cross-provider run need no configuration
    at all. The override exists because the model id alone cannot separate two
    concurrent runs of the SAME model differing on another axis: two windows of
    a sweep, a re-run under revised guidance, a second route for one id. It also
    lets an operator sidestep truncation.

    **A label is a readable stem plus a digest of the whole source, always
    both.** The stem is the id sanitised to `[a-z0-9-]`. Every name built from
    it is then legal as a directory basename and as a Postgres identifier
    alike, and an operator still recognises it.

    The digest is what makes the label injective, and it is not only about
    length. Sanitising is lossy, so it merges ids as readily as truncating
    does: `gpt-5.6` and `gpt-5-6` reduce to one stem, as does any pair
    differing only in case or punctuation. Two such runs would share a
    database, which is the whole thing the label prevents. Digesting the
    untouched source covers both causes at once. The stem cap is what leaves
    the longest possible name inside a 63-byte identifier, with `control` and a
    ten-digit repeat after it.

    The label is recorded on the run row and joins the arguments a resume is
    checked against, beside the model. It decides which databases the rows came
    out of, so resuming under a different one reads a different set of
    workspaces.

    **A post-run command reads the label off the file, never off its own
    environment.** `score`, `replay` and `report` all reach into an arm's
    database after the fact. Resolved from the environment, `score` in a shell
    pinned to one model would open another model's arms. And `report` pools a
    sweep whose runs can carry different labels. An empty recorded label is the
    unlabelled name a run created before this existed, so those files stay
    readable.

16. **No per-provider caching mechanism, now or later.** OpenAI has no
    `cache_control` and no breakpoints. Caching there is automatic prefix
    matching, and there is no charge for a cache write, which is why the
    `gpt-5.6-sol` row carries `cache_creation_per_mtok = 0.0`. So the 1.25x
    write multiplier that makes curation underwater on Opus is structurally
    absent, rather than merely cheaper.

    **A cross-provider result is therefore not directly comparable on dollars.**
    The five absolute axes are what carry the comparison. Cache creation and
    cache read stay reported apart, per round (decision 7). That split is
    exactly what tells a provider's caching behaving differently from a
    configuration behaving differently.

The eval remains a binary and never a test (ADR 0087 decision 15), and
`scripts/check-eval-not-a-test.sh` still enforces that in `make lint`.

## Rationale

**A ratio answers "which flag won", and nobody is choosing between two flags
now.** The mode is one design among many a configuration might use on its
context. The interesting question is what any configuration achieves under
pressure. An absolute number survives the next mechanism; a ratio against a
control arm does not. The control arm is still worth running, and it is worth
running as its own configuration rather than as a denominator.

**Mechanism in a criterion measures the harness, not the agent.** The route
vocabulary is the clearest case. `in-prompt` is definitionally what the control
arm scores, so half the outcome space was an arm label wearing a result's
clothes. The recovery-signature table has the same defect one layer down. It
attributes a pass to a tool call matching a regex somebody wrote. So a correct
agent taking an unanticipated route scored `unknown-pass`, and tightening the
regexes changed the reported behaviour without changing any behaviour.

**The banned-token list is a load test on the rule, not decoration.** ADR 0087
decision 5 was written down and eleven probes still shipped scoring a spelling.
A rule nothing checks is a rule that decays at the speed of the fixture. The
list is one constant, the check runs before a token is spent, and adding a term
to the list is how the rule grows.

**The failure split is behaviour and stays.** Getting a fact wrong silently is
worse than saying you no longer have it, and both are worse than asking. None of
those three names a mechanism, and the difference is exactly what a user feels.
This is the part of ADR 0087 decision 4 worth keeping, and the pass routes are
the part worth deleting.

**Pressure is the whole point, and today the fixture barely applies any.** The
longest thread runs 14 rounds, 94.5% of rounds never trim, and only two tasks
carry material that can fill a window. Every conclusion the eval has produced
describes a thread with roughly 3.5 times more headroom than it needed. The
sweep fixes that without writing bigger tasks. What matters is the ratio of
working set to budget, and shrinking the budget moves it as far as growing the
set would.

**Declaring a smaller window is safe in the direction that matters.** The
declared window only makes the engine trim earlier. It cannot make the engine
send more than the provider accepts, because the real limit is the provider's.
The declared one is always below it. So the sweep only moves downward, and
decision 11 stops it moving down past the point where the overhead eats the
budget.

**The capture makes a behavioural claim checkable.** Five runs argued about the
model's behaviour from the score file. One query against `ContextCaptured`,
grouped by section, sized the lever at 1.7% and made the argument moot. Section
names and char counts were enough for that question. They are not enough for the
next one: what the model could see when it made a particular call. Only the
payload answers that.

**The capture is the event log, because the arm is a real workspace.** An arm
is registered with the gateway, browsable, and driven over the client API by a
registered device. Its threads are ordinary threads. So the honest capture is
the rows those threads already write, uncapped. A second file format beside
them would be a second source of truth to keep in step.

**The caps are load-bearing for the product, and that is why the gate exists.**
Three consumer paths break if a normal workspace stores bodies inline. Thread
open pulls the whole snapshot in one GET with no pagination over captures. Export
passes `includeContext: true` on purpose, so a bug report stays complete. Live
SSE reaches the open client unstripped as the turn runs.

The dev workspace measures what that would cost. Its 630,888 captures occupy 352
MB stored and 10 GB of declared section size, an 82.8x blowup. The heaviest single thread
is 2,161 captures and 693 MB uncapped, which is what opening it would ship. The
message array is nearly all of it: 6,208 MB of the last 30 days against 844 MB
for knowhow and 475 MB for memory and history.

**So the gate is one environment variable on the arm's engine, and nothing
else moves.** The default stays off, the snapshot strip stays, and no retention
job joins the product for this. An arm is a disposable workspace whose database
the next seed force-drops.

**The eval's own bill is small, and it lands where the arm does.** A fourteen-task
run captures on the order of 130 rounds per arm, at roughly 100 to 130 kB each,
so about 17 MB. A four-window sweep is four of those. The bytes live in that
arm's `lucidos_eval-<slug>` database and leave with it.

**Five points, twice, and the number is borrowed on purpose.** ADR 0087's
completion margin was 5 points, argued as 0.7 of fourteen tasks. It is loose
enough for an agent that is not deterministic, and tight enough to catch a
configuration consistently dropping one task. That reasoning is about work
rather than about the flag, so it survives the bar it sat in. Using the same
margin for fidelity is a judgment, and 5 points there is 3.3 of 65 items.

## Consequences

- **The eval can no longer say "graduate" or "kill", and nothing else can
  either.** Deciding whether the mode ships is now a human reading five axes at
  four budgets. That is the honest position: the previous verdict was computed
  from a bar written before anyone had seen a run at the ceiling.
- **A single-arm run is cheaper and says less about causation.** One
  configuration at one window is about half a paired run. It cannot tell you
  whether the mode caused the number. The answer to that is to run the baseline
  as its own configuration and read two absolute blocks side by side.
- **The classifier-disagreement void and the completion-divergence void go
  dormant on a single-arm run.** Both compare two arms. Neither is deleted, and
  both still fire when a run names two arms. The classifier pin stays either
  way, because it removes a source of variance rather than of incomparability.
- **The census stops being the primary endpoint and becomes one scorer of
  four.** It was primary because a proportion out of twenty has less variance
  than twelve binaries, which is a statistical-power argument for a two-arm
  test. With no test to power, it is one deterministic scorer among several, and
  it measures the last task rather than the run.
- **Runs before this ADR are not comparable to runs after it.** The fixture hash
  moves, the outcome vocabulary changes, and the axes are new. The old results
  are kept as the record of what happened and are not rescored.
- **The capture is large and it is disposable.** Roughly 100 to 130 kB a row, so
  about 17 MB per arm per run. It lives in that arm's own database, which the
  next seed force-drops, and nothing depends on it surviving.
- **An arm's thread is expensive to open in the picker.** An arm is browsable by
  design, and a thread whose captures carry whole bodies ships all of them on
  open. That is the same cost the caps exist to avoid, taken deliberately on a
  workspace nobody works in.
- **Every run before this one recorded no bodies at all.** `capture_context` was
  never seeded, so every section carried `content: None`. Nothing from those
  runs can be replayed, and their budget-delta sums are still good.
- **`context.rs` gains a pass mask, and `ContextCaptured` a field for it.** The
  passes keep their order and their thresholds, and no trimming behaviour
  changes. `trim_passes` is defaulted, so an older row still parses and reads as
  unknown rather than as none.
- **The sweep multiplies the bill by the number of windows.** Four windows is
  four runs. Smaller windows trim more, which tends to cost more rounds rather
  than fewer, so a sweep is not cheaper at the bottom.
- **Every dollar figure the harness printed before this ADR is about four times
  too high.** The old formula priced the input total at the full rate, then
  priced both cache counts again on top. Every cached token was billed twice.
  Recomputed from the run 6 results file at the same pinned prices, the two arms
  cost $58.39 rather than $234.15. See the amendment at the end of ADR 0087 for
  the per-arm table. Those figures stay in place as the record of what was read,
  and are not rewritten as though they had always been right.
- **The correction is to the instrument and not a reprieve for the mode.** Every
  bar in ADR 0087 read a ratio, and the overstatement hit both arms, so no
  verdict moves. Cost per run goes from 1.062 to 1.107, and cost per round from
  1.043 to 1.087. What changes is the scale the argument has been had at: the
  absolute gap between the arms on run 6 is about $3, not $7.
- **A second, independent error sat under that one, and was fixed on
  2026-08-24.** The `claude-opus-5@default` row in `eval/context-mode/prices.toml`
  carried Opus 4.1's rates, so all four of its per-Mtok figures were exactly 3x
  the real ones. Every dollar this harness printed against that row is 3x high,
  double count taken out or not, and every run used it. Run 6's two arms cost $19.46, not
  $58.39 and not $234.15. Ratios are again untouched, because one factor hit all
  four line items, and the absolute gap between the arms is about $1. The second
  amendment at the end of ADR 0087 carries the table.
- **No stored token count changes, and no OpenAI row is re-priced.** The engine
  parsers are correct and are untouched. The defect was in the eval's own cost
  function, so the fix reaches new reports only. The Token Cost app already
  derives fresh input the same way and was never affected.
- **Two providers can run at once, and the operator owes three things.** Every
  provider's key has to be exported together: an arm's database is created
  empty, so its `credentials` table has no row and each provider falls back to
  its environment variable. The harness names no key itself, and a test pins
  that. The window has to be pinned with `--window`, or the two runs measure two
  budgets. And the judge must name no model in the set, which it now checks
  against `LUCIDOS_EVAL_MODEL_SET` rather than against this run alone.
- **A judge cleared against one run can still be grading its own output.** The
  check was per run, and a run knows only its own pin. With two runs live, the
  sibling's model is invisible to it. The harness cannot see a sibling, so the
  operator declares the set. Unset, the set is this run's own model and the
  check is what it always was.
- **The ceiling tasks are meaningful only at a pinned budget.** T13 and T14 were
  built to cross a 288,000-char budget derived from a 200,000-token window.
  `model_registry.rs` gives the gpt-5.5 and 5.6 families a 1,050,000-token
  window, against which those tasks cross nothing. The sweep pins the window in
  the arm's own `models` row, which `context_window_for` prefers over the prefix
  map. So the pin is a real engine-side budget, not just a smaller model.

## Alternatives considered

**Keep the bar behind an off-by-default flag.** Rejected, and it was the closest
call. Every graduation and kill condition is still computable from two arms, so
the flag would work. It would also keep about 600 lines of analysis alive. Those
thresholds were argued against a mechanism that no longer ships, and a printable
verdict is a thing people quote. A bar that nobody may cite is not worth
maintaining.

**Keep the route vocabulary as a diagnostic rather than a criterion.** Rejected.
The routes are not wrong, they are unattributable: the harness guesses which
tool call explains a pass by matching a regex it wrote. The capture answers the
same question exactly, by showing the call, so the guess has nothing left to do.

**Grow the tasks instead of shrinking the budget.** Rejected. What matters is
the ratio of working set to budget, and shrinking the budget moves it for all
fourteen tasks at once. Bigger tasks would also change what is being measured.
`T14`'s thrash loop was a trimmer bug that only looked like a task property, and
a fixture rewrite would have hidden it.

**Add a real per-request budget knob to the engine.** Rejected as unnecessary.
The registry's `context_window` already flows into the same arithmetic, and the
seed already writes exactly one model row. A second knob would be a second way
to say the same thing, and it would ship in the product to serve the eval.

**Capture to files at the Anthropic wire instead.** Rejected, and it was built
before it was rejected. The wire is where the request is verbatim, cache-control
markers and all, so it is the more faithful record.

It is also a second format and a second source of truth. It reaches one
provider, and it needs a build feature plus a directory to write to. The event
log already carries the request, the response and every tool call. The arm is a
real workspace, and its log is what a reader would open anyway.

**Lift the caps for every workspace.** Rejected on the measured numbers above.
Three shipped consumer paths would ship hundreds of megabytes, and the dev
workspace has run with capture off for months.

**A retention job that prunes old capture bodies.** Rejected as out of scope.
It is the right answer if the product ever stores bodies by default, and the
product does not.

**Score the model's own estimate of its context size, VISTA's blindness probe.**
Deferred rather than rejected. ADR 0109 names it as the one number that says
whether the panel works, and it is a good axis. It needs a task that asks the
question and a scorer for the answer, which is fixture work this change does not
do. Nothing here forecloses it: it would be a fifteenth task and a fifteenth
completion probe.

**A blinded human read of a stratified sample.** Retired with the bar rather
than rejected on merit. ADR 0087 kept it as the only instrument that sees an
unplanted regression, and never ran one. The capture is a cheaper answer to a
narrower question, and a human read remains available to anyone who wants it.
