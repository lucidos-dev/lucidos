# 0293: Apply Now refuses a thread parked on a question, and a proposal never unparks one

- **Status**: Accepted
- **Date**: 2026-09-26

## Context

A coding-agent thread asked the user a question and parked on it. Lucidos
restarted onto a new version. Recovery preserved the question, as designed.
In the same pass it re-proposed the thread's change.

The `ChangeProposed` projection wrote `'idle'` over anything but `running` and
the two verdicts. So the thread left `waiting_for_user_answer`. The question
lost its needs-attention state, and the thread offered Apply, which
`available_thread_actions` withholds from a thread waiting on an answer.

The user pressed Apply. The change had no harden marker, so Apply started a
hardening session. Its prompt overtook the question card. The user's typed
answer then took ADR 0082's supersede route and reached the hardening-only
session, which ignored it. The answer was lost with no error.

A second route reached the same place. `POST /api/v1/claude-code/apply-now`
takes only the ADR 0168 authority check. ADR 0233 keeps `apply_now` out of the
per-change gate on purpose, because it runs while a thread reads `running`.
On a live session parked on a question, it sends the review or merge prompt.
The session is blocked inside the question hook, and the prompt kills the card
too.

## Decision

Three rules, all keyed on recovery's preserve predicate,
`unanswered_question_exists_sql` (`thread_has_unanswered_question`):

1. **A proposal never unparks a thread.** `ChangeProposed` keeps the status of
   a thread parked on a question, beside a live turn and the two verdicts.
2. **Apply Now refuses a parked thread.** It answers 409 with the
   `question_open` reason before it claims a session or prompts anything. A
   check that fails refuses too, with `question_unknown`. The frontend shows
   the engine's message as "Not applied".
3. **The shared apply gate reads the question, not only the status.**
   `unsettled_thread_ids` counts a parked thread whatever its status says, and
   `change_action_refusal` asks it even when the status grants Apply. That
   covers a row the old projection already wrote to `idle`, on the per-change
   route, the `changes` tool and the bulk paths. A standing apply drops on a
   parked question on an `idle` row too, not only on a
   `waiting_for_user_answer` one.

## Rationale

**The root cause is the status, not the apply.** With the row right, the
existing rules already hold: Apply is not offered, and
`change_action_refusal` refuses a parked thread on the HTTP apply route and in
the `changes` LLM tool. The Apply Now refusal closes the one route that skips
that gate.

**One predicate.** The restart guard, the proposal, Apply Now and the shared
gate all ask the same question. A thread cannot be preserved across a restart yet unparked by
the proposal that same restart emits.

**A failed check is not a "no".** Applying over a live card is the direction
that loses the user's answer, so an unknown refuses.

**Nothing legitimate applies over a parked question.** The agent that asked is
blocked inside the question, so it cannot press its own Apply Now. ADR 0233's
reason for leaving `apply_now` ungated does not reach this case.

## Consequences

- A typed reply on a parked question stays a custom answer. No hardening run
  can overtake the card first.
- To apply over an open question, the user answers or cancels it first.
- `CodingAgentIdled` and `SessionEnded` still settle the status. Both end a
  park by design, and the status also marks a pending permission card, which an
  idle must still settle.
- ADR 0233 still holds for every other case: `apply_now` stays out of the
  per-change gate.

## Alternatives considered

**Keep `waiting_for_user_answer` on every bookkeeping arm.** Tried first. It
broke two permission-card tests: a card left behind by an idled session is
dead, and must not hold the thread. A question-keyed predicate draws the line
where the model already draws it.

**Run hardening beside the open question.** Keep the card live, hold a typed
answer until the harden run ends, then resume the original session. It needs
two sessions on one thread and a second resume path. It is not needed once
Apply cannot start over a question.

**Frontend only.** Hiding Apply already existed and was defeated by the wrong
status. ADR 0106 rejects a frontend-only gate for the same reason.
