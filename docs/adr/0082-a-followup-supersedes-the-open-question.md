# 0082: An open question is resolved by whatever arrives next, so a follow-up supersedes it rather than deadlocking the agent

- **Status**: Accepted
- **Date**: 2026-08-15

## Context

A coding agent asking a question parks *inside* the call that asked. Claude Code
blocks in the `AskUserQuestion` PreToolUse hook, Codex in its MCP tool. Only a
`UserQuestionAnswered` releases that call, and until it returns the agent cannot
reach a turn boundary, so it cannot read anything else either.

A follow-up that lands on such a thread is not always eligible to be the answer.
Two shapes are not:

- **An agent-driven message.** `message_can_answer_pending_question` requires
  `mode == Human`, so a parent's instruction or a child-completion wake falls
  through. That guard exists so a child's report is never persisted as the
  user's answer, attributed to a `thread_link` actor.
- **A message landing on an overtaken question.** Once anything in
  `QUESTION_OVERTAKEN_EVENT_TYPES` follows a question, typed text starts a fresh
  follow-up instead of answering. That is the parallel-tool-call defence
  (`docs/plans/2026-05-19-question-overtaken-defense-design.md`).

Both then routed onward to the coding-agent follow-up fast path, which emits
`CodingAgentPromptSent`. That event is itself in the overtaken set, so it killed
the card's buttons and the typed-answer route in the same stroke. Nobody could
answer the question any more, and nothing else was going to: the agent stayed
parked, and every later message piled up unread behind it.

That deadlock was observed on a live dev thread. Three follow-ups were sent over
seven minutes and none was read. The thread sat at `waiting_for_user_answer`
with a dead card.

## Decision

A follow-up that cannot be an open question's answer **resolves that question**,
as a new `AnswerKind::Superseded`, before the agent is prompted.

The rule is placed rather than conditioned. The supersede sits immediately after
the FreeText answer fast path, in the coding-agent-only block beside
`resolve_pending_permissions_as_superseded`. Reaching that line already means
this message could not have answered the question, so no further test is needed.
It uses the broad `lookup_pending_question_tool_use_id`, which ignores the
overtaken set, because an overtaken question parks the same call the same way.

`Superseded` is a distinct kind, not a reuse of `Canceled`. It skips the resume
marker and the `ContinuationRequested` exactly as `Canceled` does, since the
follow-up drives the next turn itself. It differs in the two places that face a
reader. The tool result tells the agent its question was replaced, and that the
replacement is arriving as its next input. The card reads "Replaced by your next
message".

## Rationale

The deadlock is a routing bug, so the fix belongs at the routing layer. Every
other layer is behaving correctly. The `mode == Human` guard is right to refuse
an agent message. The overtaken set is right to stop typed text being absorbed
by a question the agent has raced past. What was missing is that both of those
correct refusals left something open that only they were in a position to close.

Stated as an invariant: **a thread must never be left holding a question that
nothing can answer.** The permission lane already honours it, which is why the
supersede reads as a gap being filled rather than a mechanism being invented.

The distinct kind earns its cost twice over. To the model, "(canceled)" is an
invitation to treat the request as abandoned, when the user in fact just told it
something. To the user, a card reading "Canceled" blames them for dismissing a
question they replied past.

## Consequences

- A coding-agent thread parked on a question always resolves it when the next
  message arrives. `waiting_for_user_answer` can no longer outlive a follow-up.
- The overtaken set keeps `CodingAgentPromptSent`, so the original absorbed-text
  bug stays fixed. The two defences now compose instead of trapping each other.
- The chat lane is untouched, and the supersede is scoped to
  `use_coding_agent == Some(true)`. A chat thread queues a non-answering
  follow-up as an injection and keeps its question live, which is correct there.
- A multi-question batch ends on a supersede the way it ends on a cancel, and
  its untouched cards are padded with the same kind. The walk now also carries
  that padding into the returned answers, so the agent reads what the events
  record rather than `build_hook_answers`' `(canceled)` default.
- One asymmetry is deliberate and commented at the site.
  `arm_question_resume_if_live` fires for `Superseded` and not for `Canceled`. A
  superseded session is not being torn down. It wakes, finishes the turn it was
  in, and only then reads the follow-up, so its post-answer events need the
  re-arming a real answer's get.
- The question lane and the permission lane are now the same shape, so a future
  reader finds one pattern rather than two.
- `Superseded` rides the same persisted `UserQuestionAnswered` as every other
  answer, so it lands on the public enum. The answer endpoint would therefore
  deserialize one from a client, and it refuses that body with a 400. The kind
  asserts a follow-up arrived, which only the message router can know. A
  client-supplied one would write that sentence into the timeline and the
  agent's tool result with no message behind it.

## Alternatives considered

**Route agent-mode follow-ups as `FreeText` answers.** Would release the agent
with one deleted condition. Rejected: it re-opens exactly the bug the
`mode == Human` guard was added for. A child's `[CHILD THREAD COMPLETED]` block
would be persisted as the user's answer, carrying a `thread_link` actor, and the
user's real question would be silently consumed.

**Drop `CodingAgentPromptSent` from `QUESTION_OVERTAKEN_EVENT_TYPES`.** Would
keep the card alive and answerable after a follow-up. Rejected on two counts. It
does not fix the deadlock at all, since the agent stays parked until somebody
actually answers. And it revives the absorbed-text bug: a typed message would
again be eaten by a question the agent had raced past.

**Reuse `AnswerKind::Canceled`.** The cheap variant, and it fixes the deadlock.
Rejected because it lies twice, once to the model and once to the user, as set
out under Rationale. The saving was an enum variant, a TS union member, a card
state and two doc rows.

**Carry the follow-up's text into the tool result.** Tempting, because the agent
would get the new instruction in band rather than waiting for a turn boundary.
Rejected on two counts. The follow-up already reaches it moments later through
its own path. And putting that text in the answer slot is the attribution
problem again, wearing a different name.

**Sweep for stuck questions on a timer.** A background job resolving any
question whose thread has progressed past it. Rejected as a symptom filter: it
papers over a routing decision with a delay, and the correct moment to resolve
the question is exactly the moment the follow-up arrives.
