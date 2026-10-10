# Why the context mode destroys message-array caching

The experimental self-curated context mode (ADR 0109) froze `cache_read` at the
tools plus system tier. It paid 5.6x per token of context carried. This is the
diagnosis, the fix, and an honest verdict on whether the mechanism can pay
after it.

**Short version.** The mode rewrites, every round, the exact message that
carried the previous round's only message-tier cache breakpoint. Nothing sits
behind that edit but the system block, so the whole array is re-created at 1.25x
every round. A fourth breakpoint on the message in front of the tail fixes it,
measured. The mode then lands near break-even instead of 5.6x, and the
scratchpad size becomes the term that decides it.

## What was measured before this investigation

Thread `182bc5ce` is a within-thread, within-model control. The
`context_mode_experimental` preference flipped mid-thread on
`claude-opus-5@default[1m]`, and nothing else changed.

| | rounds | `cache_read` | `cache_creation` |
|---|---|---|---|
| mode off | 17 | grows 73,266 to 114,891 | 172 to 5,348 |
| mode on | 18 | **frozen at 50,981** | 26,017 to 42,987 |

50,981 is exactly the tools plus system tier. Costs below are base-equivalent
input tokens, counting `cache_read` at 0.1 and `cache_creation` at 1.25. On the
last round of each turn, mode off carries 116,105 context tokens for 13,007.
Mode on carries 93,968 for 58,832. That is 5.6x per token carried. Thread
`aadc79fd` shows the same shape on `claude-sonnet-5`, frozen at 51,049, so it is
not model specific.

## What mutates the array, and where each one lands

All three of the mode's instruments write into the message array. None of them
touches the system tier. They run at the top of every round, in
`engine/agentic_loop/run.rs` lines 342 to 438.

| # | what | function | where it lands |
|---|---|---|---|
| 1 | the previous panel and pad | `context_panel::collapse_tail_blocks` | in place, in an earlier message |
| 2 | tool results one round old | `context_mode::let_go_of_results_past_their_round` | in place, in an earlier message |
| 3 | this round's panel and pad | `context_panel::append_to_tail` | appended to the newest message |

Only 1 and 2 are edits. They land on the **same message**, and it is always the
message that was **last** during the previous round.

- The panel and the pad were appended to the tail during round n. At round n+1
  that message is no longer the tail, and the collapse rewrites it there.
- A tool result first seen at round n reaches age 1 at round n+1, so the round
  rule stubs it then. Those results sit in the same message.

In an array of `[U0, A1, R1, ..., A(n-1), R(n-1)]` that message is the third from
the end. `let_go_of_results_past_their_round`'s own doc comment says so: "the
result being stubbed sits two messages from the end".

## Why the breakpoint stops matching

`anthropic_wire.rs` set three `cache_control` breakpoints: the last tool, the
system block, and the last block of the last message. Only the third is inside
the array. `apply_cache_control_to_last_message` puts it on the last block of
exactly the message the next round rewrites.

So each round invalidates the prefix the previous round wrote. Both edits do it,
independently. The collapse is unconditional, because the panel rides at the
tail on every round. The stub fires whenever the previous round called a tool.
Either alone is sufficient, which is why disproving one would not have helped.

Anthropic's 20-block lookback cannot rescue it. The lookback walks back at most
20 content blocks, and it is looking for a **prior cache entry**. Entries exist
only where some earlier request wrote a breakpoint. Behind the edit there is
nothing but the system block. That is why the read froze at exactly the tools
plus system figure rather than at some partial prefix. The frozen number is the
diagnosis, because a partial prefix would land between the tiers.

## The claim in ADR 0109 that this falsifies

ADR 0109 § "The bill, and why it stopped being a veto" prices the edit at
`1.15 * S`, "where S is the invalidated suffix". It argues that S stays small
because the cut lands at the tail:

> Cutting at the tail also makes the bill small. [...] For a 40,000-char result
> the stub costs about 190 units where the cached read would have cost 4,000.

That holds only if the cache can be **read up to** the cut. It cannot: a read
needs a breakpoint written there, and there was none. In production S was the
whole message array, on every round. The ADR's model is right about what the
mechanism should cost. The wire never gave it a place to read from.

## The fix

Add a fourth breakpoint on the last block of `messages[len - 2]`, the message in
front of the tail. Anthropic allows four per request and we used three, so the
slot was free.

It is the newest position the next round leaves alone. The next round rewrites
what is currently the tail. So the prefix ending one message earlier survives
byte-identical and reads back. The wire places it structurally, with no
knowledge of the mode: `apply_cache_control_to_penultimate_message` in
`crates/lucidos-engine/src/llm/anthropic_wire.rs`.

Two engine tests hold the invariant it rests on, in
`engine/chat/process/context_release_tests.rs`. One asserts the anchored prefix
is untouched by the next round. The other asserts the previous tail **is**
rewritten, so the first cannot pass by the mutations doing nothing.

## What the live measurement says

Reasoning alone could not settle this. The wire shape was replayed against the
real Vertex Anthropic endpoint on `claude-opus-5`. The probe reproduces the
engine's per-round surgery exactly, six rounds per arm. Each arm is salted, so
no two arms can share a cache entry. Figures are round 6.

| arm | mode | anchor | parallel calls | `cache_read` | base-equivalent cost |
|---|---|---|---|---|---|
| C | off | no | 1 | 20,878 growing | 5,016 |
| F | off | **yes** | 1 | 20,878 growing | 5,016 |
| A | on | no | 1 | **7,474 frozen** | 10,121 |
| B | on | **yes** | 1 | 11,978 growing | **4,941** |
| E | off | no | 12 | **7,474 frozen** | 26,276 |
| F12 | off | **yes** | 12 | 24,387 growing | **6,826** |
| D | on | yes | 12 | **7,474 frozen** | 19,424 |

Four things fall out of that table.

- **Arm A reproduces production.** The read freezes at the tools plus system
  tier and stays there while creation climbs. Same shape, smaller scale.
- **The anchor fixes it.** Arm B reads back a growing prefix and costs 2.05x
  less than arm A by round 6. The gap widens with the array. Arm A's cost tracks
  the whole array, and arm B's tracks only its tail.
- **The anchor is free when nothing rewrites the tail.** Arms C and F are
  identical to the token on every round. The mode-off path pays nothing for it.
- **A second, older defect turned up.** Arm E is mode **off** and it freezes too.

## The second defect: a wide round breaks caching in either mode

The tail breakpoint moves two messages per round, so today's mode-off read is
itself lookback-dependent. Consecutive tail markers can sit more than 20 blocks
apart, which puts the previous entry out of range. The whole array is then
re-created. Arm E shows it: 12 parallel tool calls per round, mode off, read
frozen at 7,474 and cost climbing to 26,276.

The anchor fixes this for mode off as well, and by a wider margin. Arm F12 keeps
reading and costs 3.85x less than arm E. Mode off recovers because its anchor
only has to reach one message back. Mode on still fails there (arm D), because
its previous tail entry is dead and it must reach two messages back.

**How often it fires.** Rounds by parallel tool-call count, dev workspace,
90 days, 36,853 rounds:

| calls in one round | rounds | share |
|---|---|---|
| 1 | 35,047 | 95.1% |
| 2 | 1,635 | 4.4% |
| 3 to 8 | 167 | 0.45% |
| 9 or more | 4 | **0.011%** |

So the residual mode-on failure at 9-plus calls is four rounds in three months.
The mode-off half that the anchor now covers is the same rarity. Neither is
urgent. Both are recorded below as follow-ups.

## Does the mechanism pay after the fix?

Not on its own. It moves from catastrophic to roughly break-even. What decides
it afterwards is the scratchpad.

Let `d` be the genuinely new content each round, and `t` the suffix after the
anchor. Mode off costs `0.1 * (A_off - d) + 1.25 * d`. Mode on costs
`0.1 * (A_on - t) + 1.25 * t`. Since `t = d + panel + pad + stubbed tail`, the
difference collapses to:

> **`1.15 * (panel + pad + stubbed tail) - 0.1 * (bytes the mode has shed)`**

The probe validates it. At round 6, arm B minus arm C is 75 base-equivalent
tokens. The formula predicts 57, from a 420-token tail against 5,400 tokens
shed.

Apply it to round 18 of the production turn. The mode had shed 22,137 tokens
against control by then:

| pad | panel + pad + stub | per-round difference |
|---|---|---|
| none | 1,100 | **949 cheaper** |
| 2,000 chars | 1,900 | 29 cheaper, break-even |
| 8,000 chars, the shipped cap | 4,300 | **2,731 dearer** |

Today that same round is 45,825 dearer. So the anchor is worth roughly 45,000
base-equivalent tokens a round on that turn. It changes the ratio the mode is
judged on, rather than papering over it.

**The ordering this implies.** The pad cap is not the fix for this defect.
Capping it first would have bought a few percent of a 5.6x problem. After the
anchor it becomes the binding term. An 8,000-char pad is 3,200 tokens re-created
at 1.25x on every round, and it needs 47,150 shed tokens just to break even.
Land the anchor, then cap the pad.

**What this still does not decide.** Cost was never the mode's case. ADR 0087's
graduation bars read rounds per task and retention at the ceiling. Run 6 failed
both, for reasons that have nothing to do with caching. The anchor removes a
5.6x tax that made a fair measurement impossible. It is not an argument for
keeping the mode.

## Alternatives considered and rejected

- **Stub in bulk, every K rounds instead of every round.** This was the obvious
  candidate while the per-edit toll was `1.15 * array`. At those numbers it
  needed K near 25 to pay back. It breaks the one-round contract that the system
  prompt, the panel and the guidance all rest on. With the anchor the toll is
  `1.15 * tail` instead, so the case for it is gone.
- **Stop collapsing the superseded panel and pad.** Pure appends would cache
  cleanly, but a 40-round turn would then carry 40 panels. The accumulated reads
  alone cost more than collapsing does, and the window pressure is worse than
  the bill.
- **Render the panel into the system tier.** That invalidates tools plus system
  every round, which is strictly worse than invalidating the array.
- **Send the panel as a mid-conversation `role: "system"` message.** Available
  on Opus 5 but not Sonnet 5, and it solves nothing here. A stale system message
  is still buried by the next round, and it still has to be removed.

## Follow-ups, none of them landed here

1. **The wide-round residual under the mode.** An exact-match scheme would fix
   it: breakpoints at `messages[len - 4]` and `messages[len - 2]`, so the
   previous round's anchor is matched rather than looked back to. It needs a
   fifth slot, or dropping the tail marker under the mode. Worth 0.011% of
   rounds today.
2. **The tail breakpoint is dead weight under the mode.** The mode always
   rewrites the tail, so the entry written there is never read. Writing it costs
   0.25x of the tail for nothing. Dropping it would need the wire to know the
   mode, which is plumbing the wire currently does without.
3. **The pad is re-rendered at the tail even when unchanged**, so it is paid at
   1.25x every round rather than read at 0.1x. Leaving an unchanged pad where it
   sits would be cheaper. The cost is that it no longer rides at the tail, which
   is an ADR 0109 design question rather than a bug.

## Method note: a refused request writes no cache entry

The first version of this probe used synthetic word-salad filler. Every round
read zero cache, in every arm, including two byte-identical requests sent back
to back. The cause was `stop_reason: "refusal"`. The model declines gibberish. A
declined request reports `cache_creation_input_tokens` for the prefill while
leaving nothing readable behind.

The failure looks exactly like broken caching. Six hypotheses were tested and
discarded first: streaming against non-streaming, the 1M beta, adaptive against
disabled thinking, prompt size, the `global` endpoint's regional routing, and
propagation delay. The fix was to build the filler from real repo prose.

**So: check `stop_reason` before reading any cache figure.** A cache probe whose
prompts get refused measures nothing, and it says so in a way that reads as a
caching bug.

## Proposed ADR 0109 amendment

Text to add to `docs/adr/0109-model-writes-notes-and-sees-its-own-context.md`,
after "The bill, and why it stopped being a veto".

### Amendment: the bill was right, and the wire did not let us pay it

The section above prices a stub at `1.15 * S`, with S the invalidated suffix. It
argues that cutting at the tail keeps S small. Measured in production, S was the
whole message array on every round, and the mode cost 5.6x per token of context
carried.

The reason is not the cut. A cache is readable only at a breakpoint, and
`anthropic_wire.rs` spent its only message-tier breakpoint on the last message.
That is precisely the message the next round rewrites. It holds the panel this
round appended, and the results this round read. With nothing behind the edit
but the system block, the arithmetic above had no place to read from.

The fix is a fourth breakpoint on the message in front of the tail, which the
next round leaves alone. With it, S is the tail after the anchor, and the
section above describes what the mode actually costs.

Two things follow for this ADR. Uniform per-round expiry is affordable, so the
bulk-expiry alternative stays rejected. And the scratchpad cap becomes the term
that decides whether the mode pays. At the shipped 8,000 chars the pad needs
47,150 shed tokens to break even, where 2,000 chars needs 19,550.

`docs/investigations/2026-08-23-context-mode-cache-breakpoint-diagnosis.md`
carries the measurements. They include the arms separating this from a second
defect: a round of 9 or more parallel tool calls breaks caching in **either**
mode, because consecutive tail markers then sit more than 20 blocks apart.
