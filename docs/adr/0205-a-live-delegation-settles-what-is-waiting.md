# 0205: A Live delegation settles what is waiting, in the caller's own words

- **Status**: Accepted
- **Date**: 2026-09-17

Narrows ADR 0181's "a Live call cannot be settled by voice" to the half that is
true. ADR 0170 stands unchanged for the talker it was written for, and so does
ADR 0149's guarantee.

## Context

Two accepted decisions were right apart and deadlocked together. Neither ADR
weighed the other, because ADR 0170 is nine days older than ADR 0181.

**A Live talker holds no answering tool.** Client delegation declares no
functions, so `voice/live.rs` produces `DelegationRequested` and nothing else.
ADR 0181 read the loss as a convenience: "The card and the hangup button do
both."

**Every delegation is refused while a card is open.** ADR 0170's own
consequence. `DELEGATION_PARKED` states the fact and then says "Put that back to
them, and answer it with what they say."

So the refusal reached a talker that cannot answer, and told it to answer. One
reported call ran that loop for three minutes. The caller said what they wanted
four separate ways, the talker put the card to them each time, and nothing
reached the workspace. The trace is in
`docs/plans/2026-09-17-a-live-caller-settles-what-is-waiting.md`.

The loss was never convenience. A Live call with an open card can do nothing at
all until somebody reaches for the screen.

## Decision

**Each provider owns how ANSWERING is expressed, below the seam.** ADR 0198
settled that shape for delegation, and this is its other half. A Realtime talker
answers with the `answer` tool and an issued id. A Live one answers with the one
signal it has.

**A delegation against a parked QUESTION card, from a talker holding no
answering tool, settles that card with the caller's own words.** The card
already carries a choice meaning exactly that, and picking it sends the
transcript verbatim.

**A permission card is refused, and the note names the screen.** It takes a
decision rather than words, so there is nothing honest to send it.

**The seam grows one boolean**, `VoiceProvider::holds_the_answer_tool`, and no
tool field. Nothing above `provider.rs` learns which provider answered.

## Rationale

**Nothing here interprets anything.** The engine compares no spoken word against
a label. It presses the choice whose whole meaning is "they said something the
options do not cover", and the parked agent reads the sentence. That is the same
act a Realtime talker reaches with `#said`, and the same act the card's own
textarea performs on screen.

**Typing already does this.** `pre_emit_chat_message_received` declines to
pre-emit on a thread with an open question, "because a typed reply there is
rerouted to `UserQuestionAnswered` and never becomes a `MessageReceived`". So
speaking is catching up to typing rather than gaining a power no other surface
has.

**The talker still decides that the caller made a move.** The delegation frame
is that judgment, which keeps ADR 0170's rule that somebody thinking out loud
has not answered yet. Nothing settles a card off a bare utterance: the safety
net that fires when nobody answered the caller deliberately does not take this
route.

**A Realtime talker keeps the better path.** It can pick the option the caller
named rather than always falling through to free text. So the new route is
scoped to a talker with no channel, and reached nowhere else.

**A permission card cannot follow, and the asymmetry is the reason.** A question
card has a text slot on every surface. A permission card has one on none: it is
a boolean the agent is blocked on. Answering one out loud therefore needs a
model reading the caller's words, which
`docs/plans/2026-08-30-a-caller-answers-what-is-waiting.md` rules out in every
form, model-backed included. Weighed at approval and dropped.

**A refusal must fit the talker reading it.** The old note asked for a tool this
talker has never held. That is how a spoken assistant comes to promise the same
thing four times. Each note now names something its reader can do.

## Consequences

- **A Live caller settles a question card without touching the screen.** That is
  the defect this closes, and it is the state ADR 0170 wanted for every caller.
- **A Live caller still settles a permission card on screen**, and the talker
  says so instead of promising. Answering one out loud needs a Realtime talker.
- `DecisionResolver::doer_is_parked` becomes `parked_on`, returning what the
  doer is parked on. One read and one definition, as before.
- A question card's labelled options are unreachable by a Live caller. Their
  words carry the same meaning to the agent, which is the participant that reads
  them.
- The caller's words are spent by the answer, so no later ask runs a turn on the
  sentence that settled a card.
- Ringing off by voice is still Realtime-only. Nothing blocks a call over it, so
  it stays out.

## Alternatives considered

**Leave it, and make the refusal honest.** Tell the Live caller to tap the card.
Cheapest, and it leaves them stuck: they are on a call precisely so they need
not reach for the screen. Rejected as documenting the defect rather than fixing
it.

**A small model maps the caller's utterance onto an issued choice.** It would
cover permission cards too, which is what a full answer needs. Rejected at
approval: the plan behind ADR 0170 rules out option matching in every form, and
a question card reaches the same result with no guessing at all. The premise
does fail for a channel-less talker, so this is a live option if permissions by
voice are ever wanted on Live.

**Always deny a parked permission and pass the words along.** Safe by
construction, never grants anything, and unblocks the thread with no
interpreting. Rejected because it is wrong whenever the caller said yes, and the
agent then asks again, which rebuilds the loop one card later.

**Responses delegation, declaring our three tools to Live's backend model.** It
would restore the issued-id contract exactly. Rejected in ADR 0181 as a second
acting brain, and nothing here re-opens that.

**Settle the card off any finished utterance, delegation or not.** It would
remove the prompt's part entirely. Rejected: a caller asking "what was the
question again?" would answer it with that sentence, and a card cannot be
unsettled.
