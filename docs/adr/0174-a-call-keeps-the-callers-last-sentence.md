# 0174: The client keeps the caller's finished words, so the transcript never blanks between the speaking bubble and the engine's row

- **Status**: Accepted
- **Date**: 2026-09-05

## Context

A call and a transcript are separate things. The transcript is folded from
events, and the *live utterance* row was the one thing drawn outside that fold:
a pulse, from the moment the caller starts speaking.

The row held no text, and that was deliberate. `voice/callState.ts` said so at
the top: "A call holds no words." `docs/glossary.md` said it too. The reasoning
was that a caption nothing draws is state that can only go stale.

The engine does not write a finished utterance down straight away. `call.rs`
holds it in `pending_utterance` while the talker decides whether to answer it
or delegate it. Only then does a row exist. That wait is a model round trip,
and the client bounded it at `WORDS_BOUND_MS`, ten seconds, after which the
pulse was withdrawn.

A measured thread shows what that cost. From one spoken reply to the next
utterance's row, **47.4 seconds passed with no event at all**. The reader saw
the bars appear, the bars vanish, a long blank, and finally the words and the
reply together. The transcript was reported as dead.

The client was never short of the text. `UserTurnEnded` carries the full
transcript, and the engine sends it as soon as the provider ends the turn,
before the talker has decided anything.

## Decision

`CallState` keeps one caption: `heard`, the words of the utterance the provider
has just ended. The live utterance row carries them, so the pulse becomes the
caller's own bubble in the same frame the speaking stops.

No clock withdraws such a row. Two things retire it, and both are facts rather
than timers: the engine's own row for it landing, through the tally
`handleEvent` already keeps, and the voice session ending. A row that never got
any words is still withdrawn on the spot, which is the noise case.

The session end is the backstop, not the usual path. It exists because the
engine writes no row for one utterance: words the caller spent ANSWERING a
question card, which `call.rs` drops because the answer's own row carries them.

`ThreadState` holds a LIST of them rather than one slot, and each is keyed by
its utterance count.

**A frame landing while the caller is mid-word captions nothing.** It carries
no id and arrives well after they stopped, so one arriving then describes
something they have already finished saying. Their own frame supplies the words
when they stop.

## Rationale

The invariant we want is one sentence: **from the moment a turn begins until it
ends, the transcript is never empty.** A progress indicator, the reader's own
message, or the response is on screen at every instant, and each transition is
a swap in one frame.

A clock cannot serve that. Whatever bound we pick, the talker can take longer,
and the bound's expiry is precisely the moment the reader is left with nothing.
The only fix is to stop needing the round trip: the words are already here.

"A caption can go stale" is a real risk, and it is narrowed rather than
accepted. What is kept is a FINISHED sentence, never a partial: the provider
has ended the turn, so nothing further will revise it except a second
`user_turn_ended`, which rewrites the row in place. A reply being spoken is
still not captioned at all.

The list is what the engine's own shape requires. It holds one utterance at a
time, so a caller who carries on speaking has a second row up before the first
one's row exists. A single slot dropped the first one's words to draw the
second, which is the disappear-and-reappear this change exists to prevent.

## Consequences

- The caller's words are on screen from the instant they stop speaking, with no
  event, timer or round trip in between.
- `WORDS_BOUND_MS` no longer withdraws anything the reader was reading. It ends
  the state machine's wait, and the row outlives it.
- A hangup no longer takes a finished sentence with it. `call.rs` writes down
  whatever it holds for every end reason, so the row is owed either way.
- The row now reads as `pending`, which is the Requesting shimmer, and its
  response panel is drawn. The transcript therefore says work is under way
  during the whole of the talker's decision.
- The transcript's own empty state gained a `working` arm: a loaded thread with
  no content events and a running turn shows the shimmer, not "No messages in
  this thread".
- `docs/glossary.md` § Live utterance and § Call phase are rewritten. The
  earlier claim that the client holds no words is no longer true.
- A history load must not touch these rows. Replay walks the same `handleEvent`
  a live event does, so `applyEventRows` snapshots them across its own loop.
- `chatExchangePropsEqual` gained a term for the user bubble's text. It is the
  one row whose text moves under a stable identity, and every other term in
  that fingerprint holds across the swap.

## Alternatives considered

**Carry it in `pendingUserMessages`, reusing the typed prompt's optimistic
path.** That array reconciles by a client-minted `eventId`, and a spoken row
has none: the engine mints the id. Its safety timer also drops a message after
a timeout, which is the same blank arriving by another route. Its non-`unconfirmed`
rows flip `effectiveThreadStatus` to running, which a caller merely speaking
must not do. The per-utterance-count reconcile already solves the identity
problem and is kept.

**Have the engine emit the row as soon as it has the transcript, then amend
it.** Two rows per utterance, and the second variant depends on a decision that
has not been made yet. `SpokenMessageReceived` starts nothing while
`MessageReceived` starts a turn. So an early emit would either leave the thread
claiming a turn that never runs, or need a retraction. Events are immutable and
append-only, so there is no amend.

**Shorten `WORDS_BOUND_MS`.** It makes the blank arrive sooner. The bound is not
the defect.

**Keep the pulse up until the engine's row lands, with no text.** It removes the
blank and leaves the reader watching bars for tens of seconds with no idea
whether they were heard correctly. The words are in hand; withholding them buys
nothing.
