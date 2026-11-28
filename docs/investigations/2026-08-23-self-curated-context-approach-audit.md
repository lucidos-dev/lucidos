# Self-curated context: an audit of the approach

Read-only audit, 2026-08-23. No code was changed.

Every figure below is measured. The source is the dev workspace event store
(41,760 `main_llm` `ContextCaptured` rows, 2026-05-12 to 2026-08-23) or the
`lucidos_eval-lean-1` arm database. The last section names the query behind each
table.

## Recommendation

**Keep it.** The region is right. The timing is right. The model's judgment is
load-bearing in exactly one place.

ADR 0109 already moved the mechanism onto the live message array. It already put
the cut at the round rather than the park. Both calls are correct, and the data
backs them by a wide margin. The array is 23.5% of the input bill and 53.4% of a
round-40 request. Park boundaries can reach at most 2.0% of the bill.

**Three things are wrong with the current shape, and none of them is the
design.** The instruments cost more than the mechanism saves in short and medium
turns. The panel and the scratchpad are re-emitted at 1.25x every round. Together
they run 8.2% to 16.5% of the bill, against an 11.9% gross saving. The expiry
rule has no size floor. It stubs 53.8% of tool results by count, to reclaim 2.3%
of tool-result bytes.

**The cost argument rests on one unverified assumption about Anthropic's cache
lookback.** Do not spend another dollar on an eval until it is measured. If
Anthropic matches a cached prefix only at an explicit breakpoint, the tail cut
rewrites the whole message array every round. That is not marginally expensive.
It is a five-fold blow-up. The check is one thread with the flag on and one
`cache_creation_tokens` column.

**One thing must be said plainly, because five ADRs still rest on it.** The mode
no longer solves a loss problem. After ADR 0103, the trimmer's silent residue
reached 78 requests out of 41,760 in three and a half months. That is 10 threads
out of 1,397. Every document still arguing "the trim path loses things blindly"
cites a problem that has been fixed.

---

## The audit's target moved while the questions were being written

The brief asks whether the mechanism should move onto the region where the bytes
are. It already did. ADR 0109 landed today, supersedes ADR 0085, and is in the
tree:

- `dismiss_from_context`, the context ledger, the assembled body region and the
  arrival window are gone.
- `let_go_of_results_past_their_round` in `context_mode.rs` stubs every tool
  result one round after it arrives. It is uniform, with two exemptions: a
  `keep_in_context` keep that holds one round, and any result that errored.
- `context_panel.rs` renders a state panel at the tail of every round.
- `scratchpad.rs` gives the model a replace-whole-document note. It is capped at
  8,000 chars and pinned at the tail.

So questions 1, 3 and 6 describe a shape that no longer exists. Question 4 asks
about a call ADR 0109 made explicitly, and rejected park-only for. This audit
judges what is in the tree.

---

## 1. What problem does it solve today

**One sentence.** A long turn carries every tool result it ever read, so by round
40 most of the prompt is bytes the model already used.

That is not the problem ADR 0085 was written for. Here is what is left of the
original case.

### The trimmer's residue is pass 5, and it is nearly nothing

ADR 0103 made passes 1 to 4 leave an addressed stub. Pass 5 removes message
pairs and is the only silent loss. It is reached only after four stubbing passes
run out of material.

In production, any trim at all is rare:

| | count | share |
|---|---|---|
| `main_llm` requests | 41,760 | |
| requests with `trimmed = true` | 78 | 0.19% |
| threads with any trimmed request | 10 of 1,397 | 0.72% |

`ContextCaptured.trimmed` is `TrimOutcome::any()`, so those 78 include stub-only
trims. Pass 5's silent removals are a subset of 78 requests in three and a half
months.

The rate is that low because of a model choice, not because of the trimmer.
99.1% of production requests run a 1M-token window or larger. There
`agent_context_char_budget` is 1,488,000 chars. The largest request ever
assembled was 1,970,857 chars. The eval runs the 200k window, where the budget is
288,000 chars. That is why T13 and T14 cross it and nothing in production does.

### The second silent loss is still there and nobody has sized it

ADR 0087's amendment names two trim paths. The in-turn one is
`trim_context_if_needed`, and ADR 0103 fixed it. The cross-turn one sits in
`chat/process/run.rs` at turn setup. When the assembled sections exceed the
message budget, it calls `trim_history_from_oldest`. That drops history from the
oldest end with no stub, no event and no notice.

The path emits nothing, so the event store cannot count it. It is the only
remaining blind eviction in the engine, and it is not the one the mode replaced.
It is small in production for the same reason pass 5 is: the budget is 1.49M
chars.

### So the honest problem statement

Not loss. Growth. Everything else in a request is flat across a turn. The live
message array is not:

| round | requests | live message array (chars) | whole request (chars) | array share |
|---|---|---|---|---|
| 1 | 3,813 | 5,431 | 151,381 | 3.6% |
| 2 | 3,509 | 12,201 | 159,485 | 7.7% |
| 3 | 2,646 | 20,570 | 184,390 | 11.2% |
| 4 to 6 | 5,958 | 32,091 | 200,241 | 16.0% |
| 7 to 12 | 6,904 | 51,268 | 230,720 | 22.2% |
| 13 to 24 | 7,054 | 79,664 | 266,127 | 29.9% |
| 25 to 39 | 4,243 | 115,165 | 311,530 | 37.0% |
| 40+ | 7,633 | 215,263 | 403,105 | 53.4% |

A 40x range on the array, against a request total that does not quite triple.

**A fix to pass 5 would close nothing.** It is small, and it is not the growth.
So "kill it and fix the trimmer instead" is not available as a conclusion. There
is almost no trimmer left to fix.

---

## 2. Where the bytes are

### By region, over 41,760 production requests

| region | share of context chars | model-evictable |
|---|---|---|
| live message array (`Conversation`) | 31.4% | yes, under ADR 0109 |
| system instructions | 24.4% | no |
| tool definitions | 20.8% | no |
| loaded knowhow docs | 10.3% | no longer |
| conversation history | 4.4% | no longer |
| long-term memory | 1.8% | no longer |
| workspace inventory | 1.5% | no |
| identity and profile | 1.4% | no |
| active context | 1.3% | no |
| the request block | 0.4% | no |

The three sections the old mode governed come to 16.5% of production chars. In
the eval's lean arm it actually reached 1.71%, because they were rarely
populated. That arm's own rows show the shape at the ceiling:

| eval lean-1 request size | n | curated bodies | live message array | total |
|---|---|---|---|---|
| 120k to 200k chars | 69 | 4,101 | 14,193 | 143,703 |
| over 200k chars | 24 | 0 | 137,752 | 263,884 |

Zero curated chars on every ceiling-crossing request. Against that, 137,752 chars
of message array. The verbs governed an empty region, exactly where the mode was
supposed to earn its keep.

### The bill, not the chars

Chars are the wrong unit. A cached read costs 0.1x and a cache write costs 1.25x.
Convert every request to base-equivalent tokens, at
`1.25 * creation + 0.1 * read + 1.0 * uncached`. The total is 886.4 M, split:

| | share of input bill |
|---|---|
| cache writes at 1.25x | 48.8% |
| cached reads at 0.1x | 49.4% |
| uncached input at 1.0x | 1.8% |

Attribute each request's bill to its regions by char share. The live message
array carries **23.5% of the input bill**. That is the whole prize. It splits
unevenly:

| | requests | share of bill | array share of own bill | array share of total bill |
|---|---|---|---|---|
| warm, within-turn (gap under 5 min) | 38,652 | 71.2% | 29.8% | 21.2% |
| cold (no cache read, gap 5 min or more, or first) | 2,435 | 25.4% | 7.8% | 2.0% |

### Tool results are the array

The array is not prose. Joining `ToolResult` bodies to their turn:

| rounds in the turn | turns | array at its largest | tool-result chars | results as a share |
|---|---|---|---|---|
| 2 to 3 | 1,226 | 9,797 | 5,255 | 53.6% |
| 4 to 6 | 801 | 28,760 | 21,872 | 76.0% |
| 7 to 12 | 634 | 56,222 | 43,235 | 76.9% |
| 13 to 24 | 466 | 88,844 | 69,872 | 78.6% |
| 25 to 39 | 185 | 127,968 | 102,158 | 79.8% |
| 40+ | 197 | 228,679 | 167,940 | 73.4% |

Past round 4, three quarters of the growth region is tool results. So a rule that
governs tool results governs the growth.

### The distribution is heavy-tailed, and the rule ignores that

| result size | share of results by count | share of result chars |
|---|---|---|
| under 500 chars | 53.8% | 2.3% |
| 500 to 2,000 | 19.7% | 6.3% |
| 2,000 to 20,000 | 23.4% | 37.8% |
| 20,000 to 100,000 | 2.8% | 30.2% |
| 100,000+ | 0.4% | 23.3% |

Median result 377 chars. p99 47,878. Largest 921,341.

The biggest reads happen first. Results in the first two positions of a turn
average 7,136 chars. They carry 33.9% of all result bytes. At position 26 and
later the average is 1,548. A large early read then rides the whole turn. That is
the exact case expiry exists for.

---

## 3. Does moving the mechanism onto that region work

It works on bytes. It is close to break-even once its own instruments are priced.

### The gross saving, measured

For every production round, take the bytes ADR 0109's rule would have stubbed.
Those are the tool results that arrived more than one round ago. Reconstructed
from the `ToolResult` stream, over 90 days and 39,540 rounds:

| round | rounds | stubbable chars | saving at 0.1x | that round's bill | saving share |
|---|---|---|---|---|---|
| 1 | 3,156 | 0 | 0 | 81,994 | 0.0% |
| 2 to 3 | 5,388 | 6,273 | 251 | 14,805 | 1.7% |
| 4 to 6 | 5,676 | 24,383 | 975 | 13,363 | 7.3% |
| 7 to 12 | 6,618 | 42,171 | 1,687 | 13,541 | 12.5% |
| 13 to 24 | 6,851 | 65,731 | 2,629 | 14,924 | 17.6% |
| 25 to 39 | 4,218 | 94,738 | 3,790 | 16,312 | 23.2% |
| 40+ | 7,633 | 155,550 | 6,222 | 21,484 | 29.0% |

Both cost columns are base-equivalent tokens. Weighted over the whole period, the
saving is **99.5 M against an 839.0 M bill, so 11.9% gross.**

### The instruments cost most of it back

Two blocks ride at the tail of every round. Both are written at 1.25x.

The panel is rebuilt each round by construction. Its ages, totals and percentage
all move. At 20 rows it runs about 3,000 chars. At two rows it runs about 1,200.

The scratchpad is different. `collapse_tail_blocks` collapses the previous copy,
and `append_to_tail` appends a fresh one. That happens every round, whether or
not the model touched it. `MAX_SCRATCHPAD_CHARS` is 8,000.

| scenario | panel plus pad per round | cost as share of the 90-day bill |
|---|---|---|
| lean: 1,500-char panel, 2,000-char pad | 1,750 base-equivalent tokens | 8.2% |
| heavy: 3,000-char panel, 8,000-char pad | 3,500 base-equivalent tokens | 16.5% |

So the net lands between **plus 3.7% and minus 4.6% of the input bill**. ADR 0109
predicted cost neutrality and demoted cost from a veto. It was right, but not for
the reason it gives. The mechanism saves real bytes. Its own instruments eat them.

Break-even by turn length falls inside rounds 7 to 12 at the lean end. At the
heavy end it falls inside rounds 25 to 39. In production, 19.0% of turns run 15
rounds or more. Those
turns carry 65.7% of all rounds. The paying regime is a minority of turns and a
majority of the work.

### The cut is cheap only if one unverified thing is true

`llm/anthropic_wire.rs` places exactly three breakpoints: the last tool, the
system block, and the last message. There is no breakpoint inside the message
array.

Trace one round of the loop. At the top of round n it collapses the previous
panel and pad. It stubs the results that arrived at round n-1. Then it appends a
fresh pad and panel.

Every one of those edits lands in one message. That message was the newest when
round n-1's request went out. In steady state it sits three from the end. It is
also where round n-1 wrote its cache breakpoint. So the edit invalidates the only
message-tier entry the request could have hit.

ADR 0109 prices the edit at "about 190 units where the cached read would have
cost 4,000". That assumes Anthropic will match a shorter prefix, ending just
before the edited block. Anthropic does document an automatic lookback of roughly
20 content blocks before an explicit breakpoint. That would make the assumption
hold. Nothing in this repository measures it.

**If the lookback does not apply, the message tier is rewritten in full every
round.** At round 40 that is 215,263 chars, about 86,000 tokens. At 1.25x it is
107,500 base-equivalent, against the 21,484 a round costs today.

This is cheap to settle. It is stated as a falsification below.

---

## 4. When compression is allowed to happen

The brief's premise is that any edit behind the breakpoint forces re-creation.
The cheap moment is then one where the cache is already cold. The premise is
right about the mechanism and wrong about where it leads.

**The cheap moment and the valuable moment are disjoint.** The array is small
exactly when the cache is cold. It is large exactly when the cache is warm.

| | requests | array | whole request | array share | cache write |
|---|---|---|---|---|---|
| cold resume at a turn start | 2,124 | 4,347 | 131,469 | 3.3% | 66,940 tok |
| cold resume mid-turn | 311 | 83,395 | 259,886 | 32.1% | 116,720 tok |
| warm, within-turn | 38,652 | 85,259 | 262,487 | 32.5% | 2,535 tok |

A turn boundary already discards the array. So at a turn start there is nothing
to compress: 3.3% of the request. The only park worth compressing before is the
mid-turn one. That is a user question or a permission prompt interrupting a live
turn. There were 311 of those in 41,760 requests, or 0.74%.

Size the whole park-only design at its ceiling. Take 311 requests, 116,720 tokens
of write each, at 1.25x, of which 32.1% is the array. That is **14.6 M
base-equivalent, or 1.6% of the input bill.** Compress only tool results, and
keep the last round, and it falls under 1.2%.

Against that, per-round expiry reaches the 21.2% of the bill sitting in the array
during warm within-turn rounds. It takes 11.9% of the bill gross.

**Park-only under-delivers by roughly ten to one.** It also arrives too late for
exactly the turn the brief names. T13 and T14 cross the ceiling inside a single
turn, so no park ever happens. Production agrees. The median turn is 5 rounds,
the 99th percentile is 94, and the longest is 433. A 433-round turn with a
park-only rule compresses nothing, ever.

**The park saving is not forgone either.** Under per-round expiry the array is
already compressed when a mid-turn park happens. The smaller resume write comes
for free. All a boundary adds is a reminder to bring the notes current, and
`ScratchpadState::line()` carries exactly that as standing prose. That is the
right weight for a 1.2% opportunity.

**ADR 0109's call stands.** Read clock at the round, pay clock at the park, no
mechanism at the boundary. Its reasoning is correct, and this audit adds the
number it was missing.

---

## 5. Model judgment against a mechanical rule

The current design already answers this, and the answer is mostly "the rule
wins".

| decision | who makes it today | should it be the model |
|---|---|---|
| which result leaves | nobody: everything leaves at age 1 | no |
| when it leaves | fixed at one round | no |
| what an error does | never auto-dismissed | no, a rule |
| holding bytes one more round | `keep_in_context` | unproven |
| what a result meant | `scratchpad` | yes, and only here |

**The eviction decision is already mechanical and should stay so.** Uniform
expiry picks no victims. There is no discretion to get wrong, and no T14 to
repeat. Recency plus size plus "errors never go" would produce almost the same
set. ADR 0109's rule is simpler still: recency alone, with errors exempt.

**The note is the one place a model beats any rule.** A 40,000-char CI log
contains one retry limit. No recency rule and no size rule can produce "the retry
limit is 5" from it. Neither can an auxiliary summariser, because the compression
ratio comes from the task and not from the text. Nothing else in the industry
ships a per-item note written by the working model. That is the reason to keep
the mode rather than swap it for a trimmer tweak.

**The keep verb is unproven and cheap enough to keep for one more run.** Six eval
runs produced zero calls. All six ran without a panel, so the model could not see
how full it was. Its schema costs about 700 chars. Its failure mode is that
nobody calls it, which is what already happens. Measure it, and delete it if run
7 is zero again with the panel live.

**One thing must not be split.** The expiry needs no model discretion, so it
looks shippable with the flag off. It is not. Less Context's prune-only arm caused
premature termination in 18 runs against 3 with the note added. Expiry without a
place to write is the T14 shape: bytes gone, nothing kept, re-fetch forever. The
three pieces ship together, as ADR 0109 says.

---

## 6. The tax, and the failure mode when unused

### The fixed rent

Measured off the constants in the tree:

| | chars |
|---|---|
| `system_prompt_section`, including `NOTE_GUIDANCE` | 3,843 |
| `keep_in_context` schema | about 700 |
| `scratchpad` schema | about 1,100 |
| **total, every request of a mode-on workspace** | **about 5,650** |

That is 2,260 tokens at the measured 2.5 chars per token. It is 2.3% of an
average request. Inside a turn it is read at 0.1x, and written once at a cold
start. As a fixed cost it is not the problem.

### The variable rent is the problem, and it is not on that meter

The panel and the pad are per round, at 1.25x. Section 3 sizes them at 8.2% to
16.5% of the bill. That is three to seven times the fixed rent, and no document
mentions it. ADR 0109 records that fixed rent rose by 635 chars, and says "only a
run measures the net". A run is not needed for this half. The arithmetic is
above.

### Degradation when the model does nothing

Under the retired v3, an unused mode degraded to control plus a bounded tax.
Under ADR 0109 it degrades differently. Results vanish after one round with no
note written. That is worse, and it is the failure ADR 0103 called unsurvivable.

Three things bound it, and they are real:

- Every stub carries the `evt-<hex>` address and the `events(action="query")`
  call that reads the body back. T14's 40-char note carried neither.
- The panel states what was let go, and what a re-fetch would cost.
- Errors are never dismissed, so a failed action stays visible.

So the do-nothing outcome is a re-fetch, not amnesia. The eval must count
re-fetches. A model that curates badly then shows up as rounds spent rather than
answers lost, and rounds are what graduation condition 4 caps.

**The shape degrades safely enough to test.** It does not degrade safely enough
to default on. Keep the preference off.

### One inconsistency worth naming

`let_go_of_results_past_their_round` has no size floor. It replaces any content
longer than the stub, and the stub is about 200 chars. So a 400-char result
becomes a 200-char stub.

The panel's own `ROW_MIN_CHARS` is 500. Its elision line tells the model that
letting go costs more cache than it saves below that. The engine does the thing
the panel calls not worth doing.

The cost of that is not cache. Every stub in a round lands in one message, so the
invalidation point is fixed. Stubbing extra items there is free. The cost is
retention. Results under 500 chars are 53.8% of all results by count and 2.3% of
result bytes. The rule discards half the model's evidence by count, to reclaim
one fortieth of the bytes.

---

## 7. Product lens

**The philosophy lens does not decide this, and `.claude/rules/philosophy.md`
says so.** The test applies to a proposal adding a surface or an integration.
This is neither. It is prompt assembly inside the engine. Applying "ramps in,
never rooms" here produces noise. The rule names concurrency, recovery and schema
work as exactly the class it cannot judge.

The half of question 7 the lens does reach is whether this belongs in the engine.

**Rule 1, own the surface and rent the model, says build it.** Anthropic ships
`clear_tool_uses_20250919`. It clears the oldest tool results past a trigger and
swaps each for a placeholder. Adopting it as the mechanism would put a
provider-specific behaviour behind our own model registry.

The registry's whole job is that a model swap does not change what the product
can do. Anthropic's version also clears server-side and invisibly, so
`ContextCaptured` and the eval could not see it. Our token accounting would stop
being true.

**The differentiated half depends on something no provider has.** The address
trailer resolves against a lossless event store on the user's own machine. That
is Principle 2 made useful. A wrong drop costs a tool call rather than a
summarisation pass, because the bytes never left. Claude Code compacts because
its transcript is its only memory. We do not have that constraint, and should not
adopt the answer to it.

**The commodity half will be solved upstream, and that is fine.** Eviction
schedules are converging across providers. The note is not. ADR 0109's survey
table shows nobody shipping a per-item note from the working model.

So build the note. Keep the eviction rule simple enough to throw away.
Reconsider a provider mechanism for eviction once the panel exists.

---

## The design sketch

Keep ADR 0109's three pieces and its two clocks. Change four things. None is
structural.

### 1. Verify the cache lookback before anything else

One thread, mode on, 20 rounds of tool calls. Read `cache_creation_tokens` per
round off `ContextCaptured`.

Expected under the ADR's arithmetic: roughly 2,500 to 6,000 per round, the same
order as today's warm rounds. Expected if the lookback does not apply: 60,000 and
rising with the array.

This is not an eval. It is one thread in a dev workspace, and it costs a few
dollars. Nothing else in this list matters if it fails.

### 2. Give the expiry a size floor

Stub only results at or above `ROW_MIN_CHARS`, the 500 the panel already uses.
Justify it in the panel's own words: below that, letting go costs more than it
saves.

Effect on bytes: 2.3% of tool-result chars stay resident. Effect on retention:
the model keeps 53.8% of its results by count. Take the constant from one place,
so the panel's advice and the engine's behaviour cannot disagree again.

### 3. Re-emit the pad only when it changed

Today `collapse_tail_blocks` collapses the previous pad, and the loop appends a
fresh copy every round, unconditionally. When the pad is unchanged, that pays
1.25x for bytes already in the prompt.

Leave an unchanged pad where it is. Two rounds later it sits outside the message
being edited, and is read at 0.1x. The saving is `1.15 * pad` per round. At 4,000
chars that is 1,840 base-equivalent tokens per round. It is most of the gap
between the lean and heavy scenarios above.

The cost is that the pad drifts back from the tail and loses attention strength.
The compromise is a drift bound. Re-append when the pad changed, or when it has
fallen more than N messages behind. That keeps the attention argument, and pays
for it only when it is working.

### 4. Make the panel list what left

`select_rows` drops any item under 500 chars, and a round stub is about 200. So
stubbed items never appear as rows. They land in the elision line about cache
cost, which is the wrong sentence for something already gone. Their
`original_chars` re-fetch price is not shown either.

ADR 0109 calls the panel "the whole mitigation" for silent use-after-free. It
cannot be, while the things that went silently are the rows it hides. Select
stubbed rows by `original_chars` rather than by current size. Cap them separately
from the live rows.

### What the model sees afterwards

Unchanged in shape. A panel at the tail with ages, sizes and a percentage. A pad
it wrote. One round of full results. Stubs with addresses for everything older,
now including a row each for the large ones. Errors and one-round keeps
untouched.

The only difference: small results stop disappearing, and the pad stops being
retyped.

### What it costs

Lean instruments, a 500-char floor and a change-gated pad put the per-round cost
near 1,750 base-equivalent tokens. Against an 11.9% gross saving, the net is
positive by roughly 3.7% of the input bill. Break-even moves into the 7-to-12
round band, rather than the 25-to-39 one.

Cost is still not the reason to do this. It is the bound. The bound is now met
rather than argued.

---

## Falsification

Each is stated so it can be gathered. The first can still kill the design.

**1. The cache lookback does not reach a mutated block three messages back.**
Gather: one dev thread, mode on, 20 tool rounds, `cache_creation_tokens` per
round from `ContextCaptured`. Overturns: everything. If writes track the array
size rather than the delta, the tail cut is not cheap. Per-round expiry is then
wrong at any accuracy. Fall back to park-only, accept the 1.2% ceiling, and price
the mode entirely on attention.

**2. The note is not written.** Gather: `ScratchpadWritten` count per thread
against rounds, in the next eval run and in a mode-on dev workspace. Overturns:
the keep-it recommendation. Six runs produced zero verb calls. If run 7 produces
zero pad writes with the panel live, the bottleneck was never the interface.

**3. Re-fetches rise faster than rounds saved.** Gather: recovery-tool calls
after round 1, paired lean against control, per ADR 0087 decision 12. Overturns
the size-floor fix if the floor makes no difference. Overturns the whole expiry
if re-fetches reach the T14 shape at any floor.

**4. Retention at the ceiling goes the wrong way.** Gather: ADR 0087 kill
condition 6, lean-only silent loss over T13 and T14 against control-only.
Overturns the design as shipped. Run 6 recorded 1 against 0 here. The knowhow
notes the loss may belong to the trimmer rather than the mode. Grep the T14 lean
thread for `[Context] Context trimming` before citing it again.

**5. The panel does not fix blindness.** Gather: ask the model its own context
size mid-turn, and score the error against `ContextCaptured`. VISTA measures 0.43
to 0.84 relative error without a panel, and 0.00 with one. If our error stays
high, the panel costs 1,500 base-equivalent tokens a round for nothing. The
cheaper version is a two-line header with no table.

**6. Long turns stop happening.** Gather: the rounds-per-turn distribution,
re-run quarterly. Today 19.0% of turns reach 15 rounds, and those carry 65.7% of
all rounds. If that collapses, the paying regime collapses with it. The mode
should then be retired rather than tuned.

---

## Proposed ADR text

Not filed as an ADR file, per the brief. The caching session is writing one and
the numbers would collide. If this is filed later, it amends ADR 0109 rather than
superseding it.

### Title: the mode is priced on attention, and its instruments are priced on bytes

**Status**: Proposed

#### Context

ADR 0109 moved self-curated context onto the live message array. It set the
disposition at the round. Production measurement over 41,760 requests confirms
both calls.

The array is 23.5% of the input bill and 53.4% of a round-40 request. Between 73%
and 80% of it is tool results. Park boundaries reach at most 2.0% of the bill.
Only 311 requests in 41,760 park mid-turn with a large array.

The same measurement finds a term ADR 0109 does not price. The context panel and
the scratchpad ride at the tail of every round, and are written at 1.25x.
Together they cost 8.2% to 16.5% of the input bill. The gross saving is 11.9%.
The design is cost-neutral because its instruments consume the saving, not
because the mechanism is weak.

#### Decision

1. **The region and the clock are settled.** ADR 0109's read clock at the round
   and pay clock at the park are correct. Park-only compression is rejected on a
   measured ten-to-one margin, not only on the attention argument.
2. **An instrument is a cost, on the same meter as the mechanism.** Any block
   re-emitted at the tail states its per-round base-equivalent cost, in the
   record that adds it.
3. **The scratchpad is re-emitted only when it changed, or when it has drifted
   past a stated bound.** An unchanged pad left in place is read at 0.1x.
4. **Expiry has a size floor, and it is the panel's floor.** One constant, so the
   engine cannot do what the panel calls not worth doing.
5. **The panel lists what left, ranked by what a re-fetch would cost.** A stub
   hidden by a live-size threshold is the use-after-free the panel prevents.
6. **No eval run is authorised until the cache lookback is measured on one
   thread.** A design whose cost model is unverified cannot be tested for quality
   at four figures.

#### Consequences

- The byte case moves from cost-neutral to about +3.7% of the input bill.
- Break-even moves from the 25-to-39 round band into the 7-to-12 one.
- Half of all tool results by count stop being discarded, at 2.3% of the result
  bytes.
- The one-round keep survives one more run on probation. Six runs produced zero
  calls, all six without a panel.
- ADR 0085's open question on a note character cap is answered by
  `MAX_SCRATCHPAD_CHARS`. The cap now has a cost behind it rather than a guess.

---

## What I would change in the code

Read-only session, so nothing here was edited. Listed for whoever picks it up,
smallest first. Files are named, because the surgical-fix session is in them.

1. **`context_mode.rs`, `let_go_of_results_past_their_round`.** Add a size floor.
   Take it from `context_panel::ROW_MIN_CHARS`, not a second constant. Today the
   function stubs anything longer than its own stub.
2. **`agentic_loop/run.rs`, the panel block around line 403.** Append the
   scratchpad only when `scratchpad_written_this_turn` is true, or when the live
   copy has drifted past a stated number of messages. Today it is appended
   unconditionally, after `collapse_tail_blocks` removed the previous copy.
3. **`context_panel.rs`, `select_rows`.** Rank and threshold a stubbed item by
   `original_chars`, not by its current size. Today every stub falls under
   `ROW_MIN_CHARS` and is reported only as a count.
4. **`context_panel.rs`, the elision text.** The cache-cost line currently
   absorbs stubbed items, for which it is false. Split the two sentences, the way
   the capped branch already splits from the small branch.
5. **`chat/process/run.rs`, near `trim_history_from_oldest`.** This is the last
   blind eviction in the engine. It drops history from the oldest end with no
   stub and no event, and nothing can count it. Give it a `BUDGET_CUT_NOTE` stub
   like every pass in `context.rs`, or an event. It is small today only because
   the window is 1M.
6. **Nothing in `context.rs`.** ADR 0103's six passes and disjoint territory are
   correct. This audit found no reason to touch them.

One thing to do outside the code. The knowhow doc
`context-mode-eval-mechanics.md` still describes the v3 ledger, the three curated
sections and `dismiss_from_context` as live. It is the file every future session
loads first. Whoever owns the eval should rewrite it against ADR 0109 before the
next run.

---

## How each number was taken

Every query ran read-only against `lucidos_dev` on port 5435, except the one
marked otherwise. `events.created` is the timestamp column.

Base-equivalent tokens weight each usage counter. Creation counts 1.25, cached
read counts 0.1, and uncached input counts 1.0. Uncached is `input_tokens` less
cache read and cache creation, because `ContextCaptured.usage.input_tokens` is
the total rather than the remainder.

| table | source |
|---|---|
| region shares | `jsonb_array_elements(payload->'sections')` grouped by `group` and `name`, `producer = main_llm` |
| growth by round | round is `row_number() over (partition by request_event_id order by created, id)` |
| bill split | sums of the three usage counters over all `main_llm` rows |
| warm against cold | `cache_read_tokens = 0`, and a per-thread `lag(created)` gap of 300 seconds or more |
| results as a share of the array | `ToolResult` bodies summed per `request_event_id`, against the largest `Conversation` section in that turn |
| result size distribution | `length(payload->>'result')` over 120 days |
| stubbable bytes per round | `ToolResult` and `ContextCaptured` merged into one per-turn stream, round assigned by a running count of captures, cumulative result bytes lagged two rounds |
| trim rate | `payload->>'trimmed' = 'true'`, which is `TrimOutcome::any()` |
| eval ceiling table | `lucidos_eval-lean-1`, sections bucketed by total request chars |

Three caveats on provenance. The dev workspace is one tree of threads on one
machine, mostly Anthropic, mostly a 1M window. The trim rate in particular does
not transfer to a 200k deployment. The instrument cost uses assumed panel and pad
sizes, because no production workspace runs the mode. The two scenarios bracket
it rather than measure it. The 1.25x and 0.1x multipliers are Anthropic's, so the
shape of the argument holds for any prefix cache while the numbers do not.
