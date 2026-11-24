---
name: Coding Agent Lifecycle Events, Concrete payload shapes (continued)
description: Part of Coding Agent Lifecycle Events. Its index lists the sections it covers.
---

One part of `system-knowhow/coding-agent-events`, which lists every part.

## Concrete payload shapes (continued)

### `UserQuestionAnswered`

```json
{
  "type": "UserQuestionAnswered",
  "data": {
    "tool_use_id": "toolu_…",
    "answer": { "kind": "Selected", "option_id": "opt-0" }
  }
}
```

`answer` is a tagged union (`AnswerKind`):

- `{ "kind": "Selected", "option_id": "..." }`
- `{ "kind": "FreeText", "text": "...", "image_hashes": [...]? }`
- `{ "kind": "MultiSelected", "option_ids": [...], "text": "..."?, "image_hashes": [...]? }`
- `{ "kind": "Canceled" }`: the user closed the question without picking
- `{ "kind": "Superseded" }`: a follow-up arrived that could not be the answer, so it replaced the question

**A quiet pick ends the turn.** `quiet: true` on the event means the pick was only *quiet options*, with nothing typed. The engine derives it from the asked options, never from the client. A card whose every option is quiet is refused before it is written.

A batch of several questions carries no quiet option, and a chat card with deliveries held behind it wakes the agent anyway. Otherwise a quiet pick emits no resume marker and no `ContinuationRequested`, and the projection settles the thread to `idle` instead of `running`. A live agent stops: the Claude Code hook returns `continue: false`, the Lucidos Agent's loop ends without another model call, and Codex's tool result tells it to end its turn. The turn ends on the picked options' *quiet replies* (in option order, one per paragraph) when they have any: the engine emits them as the final reply, read from the stored `UserQuestionAsked`, never from the answer request. With no reply the turn ends empty, as before. A Claude Code or Codex agent whose process has died shows no reply, because that leg opens no turn to carry it.

**A typed answer can carry images.** `image_hashes` names the blobs the user attached, and `text` may then be empty. A coding agent reads one line per image in its answer, naming the blob's absolute path under `data/blobs/`, and opens it itself. The Lucidos Agent receives the images as image blocks after its question tool's result. An answer naming a blob the workspace never received is refused with a 409.

**Why `Superseded` exists.** The coding agent is parked *inside* the call that asked, and only an answer makes that call return. So a follow-up that cannot be routed as the answer must resolve the question anyway. That releases the agent, which reaches a turn boundary and reads the follow-up. Left open, the thread deadlocked: the follow-up's own `CodingAgentPromptSent` killed the card's buttons and the typed-answer route.

Coding-agent lane only. Any message landing on a question the agent already overtook still supersedes it. So does an engine re-entry when no agent session is live.

Two agent-driven shapes no longer supersede a question the user can still answer. A child-completion wake or an event-wait delivery waits in the agent's input queue (ADR 0255). An agent-sent message, such as a parent's follow-up, becomes a *held message* (ADR 0256). The tool result tells the agent its question was replaced, and that the replacement arrives as its next input.

The card reads "Replaced by your next message", never "Canceled". Like `Canceled`, a supersede emits no resume marker and no `ContinuationRequested`. The follow-up drives the next turn itself. Reasoning and rejected alternatives: ADR 0082.

**Only the engine writes it.** `POST /api/v1/threads/{thread_id}/answer-question` refuses a `Superseded` body with a 400 and leaves the question pending. Only the message router knows a follow-up replaced the question. Use `Canceled` to dismiss a card, or just send the follow-up.

**An engine restart alone does NOT orphan a pending question.** A thread whose newest event is an unanswered `UserQuestionAsked` is a *preserved checkpoint*. Every teardown and recovery path consults one shared predicate (`agent_recovery::thread_has_unanswered_question`) and leaves it alone. That means no boundary `ResponseAborted`, no synthetic `CodingAgentIdled`, no Continue button, and no graceful interrupt to the subprocess. Claude Code's Esc would cancel the pending `AskUserQuestion` and make the agent race past it.

The card stays answerable across the restart, and answering resumes the thread through `ContinuationRequested` then `--resume`. If anything in `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`, or the question's own `CodingAgentToolResult`, lands after the question, the card is dead. The thread then recovers as an ordinary interrupted turn (abort plus Continue).

**Answering a question the subprocess is no longer waiting for.** An answer normally travels *in band*: the engine wakes the blocked PreToolUse hook (Claude Code) or MCP call (Codex), and the answer returns as that tool's result. If the subprocess was torn down while the card was on screen, there is no blocked call to wake. The answer then emits `ContinuationRequested { reason: "answered_after_idle" }`, and the resume message carries the answer itself. `agent_recovery::continue_input_for_reason` builds it over `agent_question::answered_question_recap`: the question text, the answer, and whether the user picked an option or typed a reply.

The resumed agent's hook never re-fires. On teardown, Claude Code closes the pending `AskUserQuestion` in its OWN transcript with a rejecting `tool_result` (`toolDenialKind: "user-rejected"`, `interruptedByShutdown: true`). Its text begins *"The user doesn't want to proceed with this tool use"* and tells the agent to STOP.

That text lives in the CC binary and CC's private JSONL, outside the engine's reach. So the resume message tells the agent the closed call is a teardown artifact: **the user declined nothing, and approved nothing.** Without it, the resumed model read "card closed" plus a bare "continue" as consent, and approved a plan the user had not approved.

**Cancelling one is the mirror case, and it ends the turn.** The cancel stamps the card `Canceled`, and that `UserQuestionAnswered` moves the projection to `running`. With no agent left to interrupt, the Stop handler's settle fallback writes the terminal itself: `ResponseCanceled { user_stop }`. A live agent's interrupt emits the same, so a Cancel reads the same either way. `ResponseAborted { stale_settle }` stays for a `running` row nothing in the request explains. Apply / Discard / Archive keep it too, each carrying its own terminator.

**Pairing guarantee.** A `UserQuestionAnswered` is emitted exactly once per `UserQuestionAsked`, enforced by a unique DB index on `(thread_id, tool_use_id)`. The pair can still be unbalanced:

- The agent process dies before the user answers, the engine restarts, and the coding-agent side never revisits the question. The `Asked` row stays without a matching `Answered`. There is **no engine-side timeout that auto-emits `Answered`** for the AskUserQuestion path. The permission path has an orphan-recovery sweep, but it emits `CodingAgentPermissionResolved { allowed: false, reason: "Coding agent terminated before answering: request expired" }`, a different event family.
- The user closes the card with the cancel control. That fires `UserQuestionAnswered { answer: { kind: "Canceled" } }`, which still counts as a pair.
- A follow-up lands that cannot be the answer. That fires `UserQuestionAnswered { answer: { kind: "Superseded" } }`, also a pair, so watch for both kinds before concluding a question went unanswered.

So treat a long-pending `Asked` with no matching `Answered` as "the user is still being prompted", not as a guaranteed future event.

