# 0583: A tool result overtakes a question only when it is the question's own

- **Status**: Accepted
- **Date**: 2026-10-26

## Context

A Codex thread asked a question while a command it had started 15 minutes earlier was still running. The command finished after the question and emitted `CodingAgentToolResult`. That type sat in `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`, so the engine read the question as overtaken. The card rendered struck through. The user's typed reply missed the `FreeText` fast-path and resolved the question as `Superseded`, although the agent was still blocked on it.

The result type cannot simply leave the list. After a restart Esc'd a parked `AskUserQuestion`, Claude Code recorded the question's own rejection as a `CodingAgentToolResult`. That event is the only one that ends the park in that case.

## Decision

`CodingAgentToolResult` leaves the shared list. It ends a question only when its `tool_use_id` equals the question's own. Chat `ToolResult` stays in the list, because the chat loop asks sequentially.

The thread status follows the same rule. A result moves a thread off `waiting_for_user_answer` only when a question set the park and the result is its own. A late result of an earlier call leaves the park in place, whether a question or a permission card holds it. A permission card ends only through its resolution.

## Rationale

A result completes a call issued earlier. A call issued after the question already overtakes it through its own `CodingAgentToolCalled`. Only two results add information. One is for a call issued before the question, which is the false positive. The other is the question's own call, which is a real end. The `tool_use_id` separates the two.

One SQL helper, `ThreadEvent::question_own_result_sql`, serves the typed-text lookup, the restart preserve guard and the handoff history. The pending-change watch and the frontend (`overtakesQuestion`) apply the same pairing.

## Consequences

- A late result of an earlier call leaves the card live and typed text answers it.
- In Claude Code's parallel-tool-call race, a sibling's late result no longer kills a live card. The agent is still blocked on it, so that is right.
- Six readers now share one pairing rule, the status projection among them. A new reader of "the question ended" must use the helper.
- Both hang watchdogs spare a tool in flight past the 45-minute ceiling only when the status reads the park. A late result once flipped the status to `running`. The watchdog then killed a session that only waited on the user, and the resumed agent asked again. Meanwhile the card read "Needs your answer" under a live Stop button.
- Held deliveries no longer count a late result as agent activity.

## Alternatives considered

- **Drop the result types with no pairing.** Simpler, but it breaks the Esc'd-at-teardown case: a preserved thread behind a dead card that is never settled.
- **Pair only the typed-text lookup.** The card and the preserve guard would then disagree about whether the question is live.
