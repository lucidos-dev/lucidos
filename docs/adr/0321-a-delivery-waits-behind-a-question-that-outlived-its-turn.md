# 0321: A delivery waits behind a question that outlived its turn

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

A chat thread asked the user a question. While the question's turn is live, a
child report or an event-wait delivery cannot start a turn. It is injected into
the live turn, which reads it after the answer (ADR 0255). That queue works.

An engine restart keeps the question (`thread_has_unanswered_question`) but not
the turn that asked it. On the nightly pipeline thread, a child report arrived
right after a restart. Nothing was live to inject into, so admission started a
fresh turn. Its `ThoughtStreamed` and `ToolCalled` rows overtook the question,
the card lost its buttons, and the user's decision was never asked again.

## Decision

The chat-lane admission holds a delivery when the thread has an active question
and no live turn. A delivery is a `WaitReentry`, or an `EngineReentry` anchored
on a `ChildThreadCompleted`. The answer's resume (`resume_chat_after_answer`)
carries every delivery held since the question into its prompt. The rule lives
in `engine/chat/held_deliveries.rs`.

## Rationale

- **One gate covers every producer.** The live child fan-in, the live wait
  dispatcher and both boot sweeps all reach `process_message_with_steps`.
- **Holding writes nothing.** Both deliveries are persisted before dispatch, and
  the anchor already keeps `waiting_for_user_answer`, so the transcript already
  reads "Held until you reply". The event store stays the only state.
- **The answer is the release.** It is the one place an answer finds no live
  turn, and it already starts exactly one turn. Folding the deliveries into it
  keeps one turn per answer, the way queued messages fold into a resume.
- **Only a wait's prose needs carrying.** History rebuilds a child report
  (`build_session_messages`) but not a re-entry anchor. The note therefore
  quotes each wait and points at the child reports.

## Consequences

- A delivery never overtakes an open chat question, with or without its turn.
- A boot sweep that re-fires a held delivery holds it again. After the resume,
  neither sweep selects it, because it is no longer the thread's last word.
- A Cancel (Stop, archive) starts no turn, so held deliveries stay unread, as
  held messages do under ADR 0256. A child report still reaches the next turn
  through history. A wait's prose does not.
- A manual Continue and an answer's resume are never held. They anchor on their
  own notes. A re-sent orphan delivery keeps its anchor, so it is held like the
  original.
- A delivery that lands between the answer and the resume's admission runs as
  its own turn, and the resume injects into it. The question is answered by
  then, so no card is overtaken. This race predates the gate.

## Alternatives considered

- **Gate each producer.** Four call sites, two of them boot sweeps, would each
  need the same check. ADR 0255 rejected suppressing the callback in
  `parent_callback.rs` for the same reason.
- **Re-drive the held deliveries after the resume starts.** The resume turn
  registers inside a spawned task, so a re-driven delivery can win admission
  and run first, which is the overtake again. Folding into the prompt has no
  race.
- **Persist a hold event, like `MessageHeld`.** It adds a second record of
  something the anchor and the child card already state. ADR 0256 needed one
  because a coding-agent message has no persisted form before delivery.
