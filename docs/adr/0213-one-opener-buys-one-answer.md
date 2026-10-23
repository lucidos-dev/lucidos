# 0213: One opener buys one answer: the floor is spent by the answer it bought, never held for the call

- **Status**: Accepted
- **Date**: 2026-09-17

## Context

A reader turned caller mode on in a thread that already held a finished call.
The talker recited that call back out, in order, almost word for word.

The session ran 54 seconds. The caller said one thing. The talker then spoke for
49 seconds across eighteen turns, with silences of 0.74s to 2.3s between them.
Nothing else ran in the engine: no doer round, no delegation, no progress note.
Its last line answered the opening greeting a second time.

ADR 0211 shipped against this cause twelve hours earlier, and its floor worked.
The log shows it dropping the first two turns with "The talker spoke before the
caller did, so nobody heard it". Then the caller's greeting opened the floor.
ADR 0211 made that floor sticky, so the remaining eighteen turns went to the
caller's ear and onto the thread.

Sticky was right for the problem it solved. A Live turn ends at every 700 ms
hole in the talker's words, so one spoken answer spans several (ADR 0187). Spent
per turn, the engine's own answer went audible for one sentence and then cut
out. That reason bounds an answer. It does not license a whole call.

## Decision

**One opener buys one answer.** A call still opens with the floor shut, and the
same three things open it: the caller saying anything, the provider reporting
that they started speaking, and the engine asking the talker to speak. Each now
hands back a budget of `TURNS_ONE_OPENER_BUYS` heard turns.

**A heard turn carrying words spends one.** The last one shuts the floor, and
only an opener reopens it. It is spent at a turn END, after the row is written,
so the turn that spends it is still heard whole.

**Beside it, the block's record of the conversation is fenced.** A line above
the turns says what the record is and which label is which. A line below says it
ends there, that every line in it was already heard, and that none of it is to
be said again. The labels drop their verb: `Them` and `You`, not `They said` and
`You said`.

## Rationale

**The recitation is the cause, and the budget is the guarantee.** The block is
what the talker performs, so the fence attacks the cause. A fence is a tendency,
and this model had already ignored the block's heading and survived its move to
the quiet channel. ADR 0211 set that standard for itself, and the budget is what
meets it.

**The bound is counted in turns because no silence window separates the two
cases.** The recitation's longest silence was 2.3 seconds. A real answer on the
same thread paused for eighteen seconds mid-way. A window short enough to cut
the first truncates the second, and a truncated answer is a worse failure than a
long one. Turn count does separate them: the longest real answer in that thread
ran to six turns, the recitation to eighteen.

**The reset is what keeps a working call away from the bound.** A caller who
speaks every few seconds keeps buying answers, so the bound is only ever met by
a talker speaking to nobody.

**Muting is the only lever there is.** A Live talker cannot be cancelled, so the
engine cannot stop the words. What it can do is refuse to relay them, which is
what the floor already was.

**The closing line is the load-bearing half of the fence.** The defect ADR 0211
recorded is the record's last line reading as a turn nobody answered. Anything
that follows that line removes the reading.

## Consequences

**A real answer longer than the bound is cut short.** The caller hears it stop,
and one word from them resumes the conversation. That is the trade: an answer
truncated at eight turns against a monologue with no end.

**A muted run is greppable**, by one log line naming the bound. It sits beside
ADR 0211's line for a floor that never opened.

**A turn the bound muted is still billed, and still disarms nothing.** Both
follow ADR 0211 unchanged. We were billed for it whoever heard it. And a turn
the caller never heard must not disarm the bound that sends an unanswered
caller to the doer (ADR 0185).

**There is no doer fallback behind the bound, and none is owed.** By the time it
bites, the caller has heard eight turns. So ADR 0185's bound was disarmed by the
first of them, and nothing re-arms it. That is the honest shape: this bound
prevents a monologue, and ADR 0185's prevents silence. A caller cut off
mid-answer heard an answer, and one word from them buys the rest.

**The Realtime provider inherits the bound and will not meet it.** It speaks
only on a response it was asked for. The gate sits above the seam, so a provider
swap cannot reopen this.

**The block grows by the fence, and recall grows with it.** The two are separate
currencies, so neither pays for the other. `THREAD_RECALL_BYTES` bounds the turns
alone, so the fence's two lines are about 270 bytes on top of it. Inside that
budget, `Them` and `You` are five bytes a turn shorter than the labels they
replace, so more turns fit.

**The talker still remembers what it recited.** The provider holds those words
in its own session history and no seam member can retract them.

## Alternatives considered

**A silence window: shut the floor after a hole longer than a breath.** The
first idea, and the measurements killed it. See the Rationale: the recitation
paused for 2.3 seconds and a real answer for eighteen, so the two orders are the
wrong way round.

**Match the talker's words against the block and drop a recitation.** It attacks
the cause exactly, and it needs a similarity threshold: the replay was
paraphrase as often as quotation. It would also cut a caller who legitimately
asks what was just said. Rejected as more machinery and more failure modes than
a count.

**Lower `THREAD_RECALL_BYTES` so there is less to recite.** The raise to 4,000
earlier the same day is what put the whole earlier call in the block, so it is a
contributing factor. It bought recall a reported call needed, and trading that
back is a separate decision with its own evidence. Deferred rather than refused.

**Drop the conversation turns from the block.** It ends the recitation outright
and costs the one thing the block exists for. Voice would answer "what were we
saying" with a delegation and a wait.

**Reword the block and tell the talker to wait.** ADR 0211 weighed and rejected
this as the whole fix, for the reason that still holds. The fence here is that
idea kept as the cheap half, under a mechanism.
