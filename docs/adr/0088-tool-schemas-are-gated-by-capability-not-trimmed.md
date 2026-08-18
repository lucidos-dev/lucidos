# 0088: A tool family is offered only where the workspace can use it, and nothing else in the two fixed blocks is trimmed

- **Status**: Accepted
- **Date**: 2026-08-18

## Context

ADR 0085 closed on an unowned item: "The two largest fixed blocks were never
examined. The tools array is 113 schemas on every request of every thread, and
the system prompt is about 22.9k tokens. Both are read at 0.1x every round and
neither came up."

They are now measured, in
`docs/investigations/2026-08-18-tools-array-and-system-prompt-economics.md`,
over 3,437 Opus `main_llm` calls in 7 days. Structural claims are re-checked
across 1,423 turn boundaries in 30 days.

| | measured |
|---|---|
| Tools array | **27,175 tokens**, 72 schemas, 69,456 wire chars |
| System prompt | **21,668 tokens**, 58,453 chars |
| Both, against a 113,899-token mean prompt | **42.1% of every request** |
| Both, as cost | **$147.52 of $459.12, 32.2% of the 7-day bill** |
| Floor if every call read them at 0.1x | **$82.19, 17.9%** |
| Miss surcharge above that floor | $65.33 |

0085 was right that this is the largest unpriced item: 32.2% against the turn
boundary's 25.0%. It was wrong on two facts. The array is **72** schemas, not
113. And a coding-agent thread pays none of it, because its Claude Code calls
carry Claude Code's own tools.

Neither size is an estimate. Anthropic's first `cache_control` marker sits on
`tools[-1]`, so a read that stops there IS the tier's token count, stated by
the provider. Across 15 engine builds every build reads one exact value, low
equal to high, at 2.5567 wire characters per token.

Three facts shape everything below.

- **$82.19 of the $147.52 is a floor no cache fix can reach.** It comes down
  only if the blocks get smaller. Marginal value of size is $1.09 per 1,000
  characters off the tools array per week, and $1.27 off the system prompt.
- **The array has no fat head.** The largest schema is 4.3% of it and twenty
  schemas carry half. There is no trim that both moves the number and survives
  review.
- **The remaining $65.33 is the miss, and it is not a size problem.** 58.6% of
  turn boundaries read nothing at all, against an array that is byte-identical
  on every thread and every workspace of that build.

## Decision

Five parts. One changes behaviour, three foreclose tempting alternatives, and
one closes a number that was never real.

1. **A tool family is offered only where the workspace can use it.**
   `generate_image` already works this way, gated on an image provider being
   configured. Extend that pattern to the two families the measurement found
   unusable. The five email schemas gate on the workspace having an email
   account, and `execute_intent` gates on it having an intent. That is 4,314
   characters of the 69,456.
2. **The gate is a function of workspace configuration, never of the thread.**
   Every thread in a workspace sees the same array, so the cross-thread warmth
   that is actually measured survives untouched. A per-thread-kind array is
   rejected below.
3. **No prose is trimmed, from a tool schema or from the system prompt.**
   The redundancy this was supposed to reclaim was measured and is not there.
4. **Lazy tool disclosure is rejected in the shape that adds fetched schemas to
   the tools array.** The arithmetic loses by more than two to one.
5. **`ENGINE BUILD` and the client-URL sentence leave the cached system tier**,
   the way ADR 0084 moved the clock. Both are volatile values in a tier whose
   guarantee is that it holds none.

The `~9,200 unmatched tools tokens` line in `docs/temporary-measures.md` is
retired as an arithmetic artifact. The `prompt-cache-first-of-turn-miss`
investigation keeps the real question, and its remaining fact is the 58.6% of
boundaries reading zero.

## Rationale

**Capability gating is the only cut that is both principled and free of a
quality argument.** A schema the workspace cannot act on is not terse guidance
the model might need. It is an offer the engine will refuse. The workspace
measured here has zero email accounts, and in 90 days its agent has never
called `configure_email`, `send_email`, `read_email` or
`save_email_attachment`. It has zero intents, and has never called
`execute_intent`. `generate_image` proves the mechanism is already accepted.

**Gating per workspace keeps the sharing that the data shows matters.** The
prior artifact measured 26.6% of boundaries reading a warm prefix that a
*different thread* in the same workspace had kept alive. A workspace-level gate
leaves every thread in a workspace on one array, so that survives. What it
forfeits is cross-*workspace* prefix sharing on one credential, which is
unmeasured here and bounded by two known facts. The array already differs by
engine build, nine distinct sizes in these 7 days. And 58.6% of boundaries
already read nothing, so the universal prefix is not doing much work at the
moment it would be needed.

**Prose is not redundant, and the measurement says so rather than a
preference.** The system prompt shares 0.36% of its 8-word shingles with the
tool schemas, and 0.00% at 20 words. All eighteen shared runs are one topic:
the `ask_user_question` ban on an "Other" option. The tree mirrors that on
purpose and pins it with a test in each place.

Against the engine-shipped knowhow corpus, 26 files and 1,047,678 bytes, the
overlap is 0.30% at 20-word shingles. It concentrates where a routing rule
shares vocabulary with its destination, which is what routing looks like. There
is nothing said twice worth reclaiming.

**Nobody has measured a quality cost of shorter schemas, and the tree already
records the cost of shorter schemas going wrong.** Every row in
`PER_TOOL_CEILING_EXCEPTIONS` names a structure that cannot shrink, and most
name a failure that reached a user. `run_coding_agent` repeats the
source-checkout precondition twice because "a packaged install's agent believed
the unconditional wording and narrated a spawn the engine refuses".
`await_event` carries thirteen phrases pinned by four tests, "each one a
failure that reached a user". Recommending a prose cut means recommending the
reversal of fixes with known incident provenance, on no evidence. The quality
question is untested in both directions, and the direction with recorded
incidents is the one to leave alone.

**Lazy disclosure loses on cache mechanics before any quality argument, because
the tools array is the FIRST cache segment.** Adding a schema mid-turn changes
that segment, which forfeits every tier behind it. That is the whole
113,899-token mean prompt at 1.25x, or **$0.71 per fetch**. Hiding roughly
48,400 characters saves 18,930 tokens per call, which is $0.0095 at the read
rate and $32.48 a week here. At a top-20 resident core, 645 of 953 threads in
90 days would need at least one fetch. That is 1,419 distinct dereferences, so
roughly 110 fetches a week, or $78 of cost against $32 of saving.

**The count is not the cost, and the usage tail is wide.** 61 of the 72 tools
were called in 90 days. The eleven that were not are 9.3% of the array, and
more than half of that is the two unusable families this decision gates.
`send_notification` is 214 calls across 207 threads, so a tail tool is
typically used once by almost every thread that uses it. A scheme that hides
the tail touches most threads.

**ADR 0084's rule is load-bearing and two values break it today.** The rule:
the system block "holds nothing that varies per turn or per thread". It is "a
function of workspace state and preferences, and of nothing else".
`engine_build_section` is a function of live build state, and its own comment
says it is "rebuilt every turn, never stale". Its four states are 327, 359, 371
and 374 characters.

In the 7-day data, two threads in one hour and one build repeatedly disagree by
exactly 44 characters. That is `update_available` against `current`. The
client-URL sentence reads `self.frontend_origin`, a runtime value set from the
last observed request origin, which is neither workspace state nor a
preference.

Neither trips `two_threads_in_one_workspace_share_the_system_block`, because
that guard compares two threads at one moment and both read the same current
value. The guard is not wrong. It answers a per-thread question, and these are
per-turn values.

**The ~9,200 was two arithmetic artifacts, not a phenomenon.** Over 30 days and
1,423 boundaries, no read ever landed between zero and the full tools tier. The
22,659 that the shortfall was computed from is a bucket mean over 36.7% zeros.
The non-zero reads in that bucket average 35,581, above the tier rather than
below it. The $0.058 came from a three-way residual of the boundary write,
while the artifact's own direct subtraction gives 4,516 tokens and $0.028. The
registry already carried the contradicting sentence, that reads are
all-or-nothing and nothing lands between zero and the 22.4k tools block.

## Consequences

- **The tools array becomes workspace-shaped.** It stops being byte-identical
  across installs on one credential. That is the price of decision 1 and it is
  accepted with the sharing loss unmeasured.
- **A new tool family must state its gate, or state that it has none.** Without
  that, gating is a one-off for email and intents rather than a rule, and the
  next unusable family arrives resident.
- **A gate is a cache event.** Configuring the first email account rewrites the
  tools tier for that workspace, at 27,175 tokens times 1.25x once. Adding a
  capability is rare, so this is a one-off per capability rather than a
  per-turn cost.
- **The saving is small and honestly so: $4.70 a week in this workspace.** It
  is taken because it is the only cut that costs nothing in quality, not
  because the number is large. Anyone hoping for a large number should read the
  floor: $82.19 a week is structural.
- **`always_loaded_context_stays_under_budget` sits at 108,022 of its 108,050
  ceiling.** With 28 characters of headroom, the next schema addition breaches
  it. Gating removes 4,314 characters from the live array but NOT from that
  test, which measures the engine-authored surface rather than what one
  workspace is sent. Decide deliberately whether the ratchet follows.
- **Two ADR 0084 fixes fall out**, both the shape 0084 already established: the
  value moves to the message tail and the system prose points at it. The
  `ENGINE BUILD` state is more awkward than the clock, because it is guidance
  rather than a reading.
- **The `prompt-cache-first-of-turn-miss` investigation gets a sharper question
  and loses a wrong one.** Partial prefix matching is ruled out by 1,423
  boundaries with zero partial reads, so the divergence hypothesis in its own
  experiment note is dead. The miss is a lookup failure.
- **The post-0084 window is 9 boundaries and settles nothing.** Five of them
  read tools and system together against 3.7% before, which is the predicted
  direction and nothing more. Re-measure before pooling anything across
  2026-08-17 16:29.
- **The dollar figures are one heavy dev workspace at $65.59 a day.** The
  shares port, the dollars do not.

## Alternatives considered

**A different tools array per thread kind.** Rejected, and the measurement
removes its premise. Over 7 days every one of 3,639 `main_llm` calls carried
the same 72 schemas in the same order, one shape hash across 84 threads.
Triggers are 245 of 3,437 calls, so the only variant with money in it is the
chat one. No tool is demonstrably unneeded by a chat thread. A per-kind split
also fragments the FIRST cache segment, where a sparse thread kind would write
its own cold prefix at most fires.

**Lazy tool disclosure with fetched schemas joining the array.** Rejected on
the arithmetic above: $0.71 per fetch against $0.0095 per call saved. The
stated failure mode is real too, and it is the one the question named: a model
that does not know a tool exists cannot ask for it. A discovery tool that
enumerates what is available is a resident routing list again. That is the
shape ADR 0086 kept for knowhow, and its cost lands back in the same tier.

**Lazy disclosure where fetched schemas return as tool-result TEXT.** Deferred
rather than rejected, and it is the survivor. The tools array never changes, so
the first cache segment is safe, and the cost is one extra round at $0.108. At
12.8 rounds a turn, a 5,000-char bundle fetched at round 2 rides the message
tier for about eleven re-reads, roughly $0.011. It needs a generic dispatcher,
because a tool the provider does not know cannot be validated or called by
name. That is a larger design than this decision, and it should be argued on
its own once the miss is understood.

**Trimming tool-schema descriptions.** Rejected. The overlap measurement leaves
no duplication to reclaim, and `PER_TOOL_CEILING_EXCEPTIONS` documents that the
prose in the largest schemas is where user-visible failures were fixed. The
`print_frozen_tool_contract` diagnostic already exists, so a prose-only trim can
be proved not to change the callable contract. The tooling is ready and the
evidence to justify using it is not.

**Trimming the system prompt.** Rejected on the same measurement. Two thirds of
it, 40,642 characters, is constant on every workspace on earth and is the
engine's own instruction set. The workspace-shaped quarter is the apps list and
the knowhow routing list. ADR 0086 has already ruled on both, and this decision
does not touch them.

**Moving the system prompt's constant two thirds into a separate cache
segment.** Rejected: Anthropic allows four `cache_control` breakpoints and three
are in use, but a fourth here buys nothing. The system block is already one
segment ending in one marker. A split would only help if the workspace-shaped
tail changed while the constant head did not. Post-0084 the whole block is
already stable within a workspace. Across workspaces the constant head is
identical anyway, so a split protects a prefix that already matches.

**Deleting the eleven never-called tools outright.** Rejected. Six of them are
unusable here rather than unwanted, which is what decision 1 addresses without
removing capability. `browser_type` at zero calls beside `browser_open` at 131
is worth someone's attention. But a 463-character schema does not justify
removing a way to type into a page.

**Measuring the cross-workspace sharing loss before gating.** Rejected as the
gating condition, though the measurement stays available. It would delay a
decision worth $245 a year here to price a loss bounded by two known facts: the
array already differs per engine build, and most boundaries already read
nothing.

**Leaving the two ADR 0084 violations alone because the guard passes.**
Rejected. The guard answers a per-thread question and these are per-turn
values, so passing it is not evidence of compliance. 0084's whole point is that
a fixed-width value inside a cached tier is invisible to every instrument
except the cache itself.
