# 0666: A call opens listening

- **Status**: Accepted
- **Date**: 2026-11-12

Amends [ADR 0211](0211-a-call-opens-silent.md),
[ADR 0213](0213-one-opener-buys-one-answer.md) and
[ADR 0218](0218-a-mute-covers-a-sentence.md).

## Context

Starting or reactivating a call made the talker speak first. It opened with a
summary, or answered the previous call's last line, before the caller said a
word. Two calls on one evening showed it:

- One call relayed eight rows of the record over silence. The first was a doer
  answer written two seconds after the previous call hung up.
- The next call answered the line that previous call ended on. It also spoke a
  doer answer to a turn started on the previous call, unprompted.

Three causes, each enough alone:

1. **A wordless edge opened the floor.** ADR 0218 let the device's speech gate
   open it, so a Live talker answering before its transcriber reports a word
   is heard. A breath, a cough or the room trips that gate too. The opener
   bought eight turns (ADR 0213), and the talker spent them on the record.
2. **A stale answer was spoken.** The call spoke every doer answer on the
   thread, and speaking is itself an opener. An answer to a turn begun on the
   previous call opened the floor on this one.
3. **The record ended on something that read as owed.** An answer written
   after a call ended, or the caller line a call ended on, read as a turn
   nobody answered.

Every activation is a new provider session. A dropped socket ends the call, so
there is no reconnect path to treat differently.

## Decision

**A call opens listening, by mechanism:**

1. Until the caller's first words on this call, the device's sound does not
   open the floor. It holds the talker: its audio, words and turn ends are
   kept, neither played nor dropped. The caller's words open the floor and
   release the held output in order. A held sentence that ends with no words
   is decided unheard and logged as ADR 0211's floor line is. After the first
   words, the device's sound opens the floor as ADR 0218 has it.
2. The provider's own `speech_started` still opens the floor at once.
3. An answer, failure or Stop for a turn begun before the call opened goes to
   the talker silently, as an `[EARLIER]` note. The call tells such a turn by
   whether its starting event arrived on the call's own subscription. A
   `PromptInjected` naming it counts only when it injects a message this call
   saw, such as the call's own delegation. An engine note injected into the
   turn does not. An unstamped terminal is spoken as before.
4. The record labels an answer written after a call ended, and a caller line
   a call ended on, as not owed. The Live opening says the call has just
   opened and that a recap is given only when asked.

A log line names the opener each time the floor opens.

## Rationale

**Hold, never drop.** ADR 0218's case is real: the Live talker answers
"Please?" before its transcriber reports the word. Dropping the talker's output
before words would mute that answer again. Holding it costs the caller the
transcript's lag on their first answer, and nothing else.

**Words decide the hold, as they decide the audience** (ADR 0211). A sound
says somebody made a noise. Only words say the caller is talking to us.

**The provider edge opens.** Realtime replies only to caller audio it has
committed, and its completed transcript can land after that reply ends. Held
there, a short first reply would end held and be dropped. Live states no such
edge, so the hold covers Live fully.

**The turn's start, not a clock.** A turn begun on this call is one the call
saw begin. A typed message, a delegation and an orphan re-run all emit their
start on the thread. No time window separates a stale answer from a fresh one.

**Mechanism first, prompt second.** Decision 4 is a tendency on top of 1 to 3,
never instead of them, the standard ADR 0211 set.

## Consequences

- Nothing the talker says before the caller's first words reaches anybody,
  unless the engine itself hands it something to say.
- The first answer on a Live call can arrive late by the transcriber's lag.
- An answer that finished between calls is not announced. The talker knows it
  and passes it on when asked.
- A recap needs asking for. The talker still holds the whole record.
- Held audio is bounded at about twenty seconds, as a memory bound only. Past
  it, the hold is decided unheard.

## Alternatives considered

- **Drop talker output before the first words.** Rejected: it mutes the first
  answer again, which is the failure ADR 0218 fixed.
- **Hold on both edges.** Rejected: it drops Realtime's first reply whenever
  the transcript lags that reply.
- **A silence window after call open.** Rejected for the reason ADR 0213 gave:
  no window separates a recitation from a real answer.
- **Matching the talker's words against the record.** Rejected by ADR 0213.
- **Prompt-only: tell the talker to wait.** Rejected: ADR 0211 measured that a
  Live talker answers the record's last line regardless.
