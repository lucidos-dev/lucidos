# 0191: A pause spends nothing: it ends no row on either side

- **Status**: Accepted
- **Date**: 2026-09-16

Amends [ADR 0188](0188-one-thing-the-talker-said-is-one-row.md), which is right
that a reply is one stretch and wrong that a pause may still spend the caller's
words. Carries its rule to the last two places that read a 700 ms hole in the
talker's words as a move of the conversation.

## Context

ADR 0188 kept the caller's row flush at the pause, and justified it in one
sentence: "The pause is the only signal that the talker is not about to
delegate." One morning of calls falsifies it.

| Time (UTC) | Event |
|---|---|
| 06:15:07.155 | `SpokenMessageReceived` `Yeah` |
| 06:15:08 | log: `[Voice] The talker asked for the doer` |
| 06:15:35.534 | `VoiceSessionEnded` |

No `WorkDelegated`, no turn. The caller said `Yeah`, the talker said `Okay.`,
and the hole after that one word wrote their words down and took them.
`settle_the_pending_utterance` needs a held utterance AND a held ask, so the ask
one second later paired with nothing and waited until the hangup. The talker had
been acknowledged `Taken.`, so it truthfully told the caller it was on it.

Saying a word and then asking is the ordinary shape on a Live call, and it
happened three times in a row. The provider makes it worse: `live.rs` hands the
caller's words over on the talker's first worded delta, so the delegation frame
carries none either.

The client had the mirror image of the same mistake. `callState.ts` read a
`talker_turn_ended` frame as the end of a reply, emptied its buffer, and opened
a new bubble on the next delta. One reply had four measured holes, so it drew
four bubbles where `call.rs` writes one row. The reader watches sentences
disappear as they are spoken. ADR 0188 promises the live row and the persisted
one "render identically", and they did not.

The full trace is in `docs/plans/2026-09-16-a-pause-spends-nothing.md`.

## Decision

**A pause spends nothing.** `TalkerTurnEnded` keeps the floor, the per-turn
delegation reset, the queued-answer drain and the overheard offer. It stops
taking the caller's held words.

**The caller's row is written when the conversation moves**, which is the rule
ADR 0188 already gave the reply: the caller's next finished words, a new thing
handed to the talker to say, or the call ending. `say` writes it before the
reply's, so a question reads above the answer to it.

**A live caller row whose words are FINAL reads as settled**, and only the pulse
and a partial still shimmer.

**The live reply bubble accumulates the stretches of one reply.** It is
assembled in `store/liveUtterance.ts`, the one seam between a call and a
transcript, and it keeps its id and its moment across every pause.

**The engine's own row retires the live one**, detected as a rewrite finding no
row standing. That is the claim `writeRow` already reads on the caller's side.

## Rationale

**Both sides of a call now follow one sentence.** A row is one thing somebody
said between two moves, and a pause is not a move. Every place that still
treated it as one produced a defect: eleven bubbles for one reply (ADR 0188),
then a lost ask and a restarting bubble here.

**The pause could never have been the signal ADR 0188 wanted.** The talker's
tool call and its speech come from the same model on one socket, in whatever
order it composes them. A pause proves it stopped speaking; it proves nothing
about what it is about to ask for.

**Writing the row at the pause was a choice made too early.** The row's TYPE
depends on what the talker does: delegated, the words are a `MessageReceived`
that runs a turn; answered alone, a `SpokenMessageReceived` that runs nothing. A
`SpokenMessageReceived` is `Metadata` and can anchor no turn, so once it is
written the ask has no way to start one with those words.

**The shimmer moves to where the shimmer is drawn.** ADR 0188 kept the pause
flush to stop a "Requesting" chip standing for a whole reply. That chip is a
client verdict on a client row, so the client is where it is answered: a
sentence the caller has finished is not something the reader is waiting on.

**The row's unit belongs to the bridge, not the reducer.** `callState.ts` is a
state machine over frames, and `said` honestly means the turn being spoken. The
ROW is a transcript concept, so it is assembled where the transcript is drawn.

## Consequences

**A caller's row is written down late**, the same trade ADR 0188 took for the
reply. The live row covers it on screen. A trigger watching
`SpokenMessageReceived` sees it late.

**An ask can now claim words the caller spoke before the talker's last pause.**
That is the point, and it is bounded: `settle_the_pending_utterance` takes the
utterance, so one utterance wakes the doer once however many times the talker
asks.

**The live bubble and the persisted row now carry the same text**, so the swap
between them moves nothing on screen.

**One narrow case still duplicates a bubble.** A persisted row landing while the
bridge holds stretches it does not cover leaves those stretches drawn twice
until the next move. It needs the engine to flush without the client seeing a
pause first, which a barge-in is the only route to.

## Alternatives considered

**Let a late ask reclaim the words the pause already wrote down.** Rejected: the
sentence would be on the thread twice. One thing the caller said is one row
(ADR 0185), and this would be two: the spoken row, and the message anchoring the
turn. Only a `Start`-class event can anchor one, so there is no way to keep the
written row and start work with it.

**Hold the caller's row on a short timer past the pause.** Rejected: it is the
silence timer this whole design refuses, applied to the talker instead of the
caller. The measured holes inside one reply reach 3.6 s.

**Keep the client's reply unit and coalesce the bubbles when they render.**
Rejected: the bridge already holds the row's identity, so a second rule in the
renderer would be the same fact stated twice. ADR 0188 refused the mirror of
this on the engine side.

**Reset the client's buffer from the persisted row over SSE.** Rejected: it
crosses the transcript into the call state machine, and the two transports give
no ordering. The bridge can read the claim from the store it already writes to.

**Send a frame when the engine closes a reply row.** Rejected as unnecessary
rather than wrong: nothing the client needs is missing, since it already
receives the row itself.
