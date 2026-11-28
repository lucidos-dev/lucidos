---
name: Coding Agent Lifecycle Events, Question vs. permission: why both event families exist
description: Part of Coding Agent Lifecycle Events. Its index lists the sections it covers.
---

<!-- domain-event: CodingAgentFailureObserved: The worked failure-monitoring script emits this domain event. -->

One part of `system-knowhow/coding-agent-events`, which lists every part.

## Question vs. permission: why both event families exist

Four user-facing prompt mechanisms ride on two event lanes:

1. **CC's native `AskUserQuestion` tool.** CC calls it for structured input mid-turn ("which approach", "is this OK to do"). Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = claude_code`. The `question` payload field is the literal prompt text CC supplied.
2. **Codex's `ask_user_question` MCP tool.** Same wire shape and QuestionCard, one question per call, raised from a Codex session via the `lucidos` MCP server (`lucidos mcp-permission-server`). The MCP server long-polls the endpoint CC's hook uses and returns the answer as the tool result, so the Codex turn continues in place. Fires with `meta.channel = claude_code` (the coding-agent channel). **Codex-only, and enforced**: CC spawns the same binary with `--permission-only`, so a CC session lacks the tool. In CC, every MCP tool call would cost a permission click.
3. **Chat agent's `ask_user_question` tool.** Same wire shape and QuestionCard, raised by the in-process chat agent. Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = chat`. Its optional `message` arrives first, as one `TextStreamed` in the asking turn, just before the first card. The tool blocks on the question wait registry, then returns the joined `{question_text: label}` map as the tool result on the same turn. Use it from chat threads when a button-driven answer beats typing (see `crates/lucidos-core/src/llm/tools/misc.rs` § `ask_user_question_tools`).
4. **Coding-agent permission prompts.** Authorization for one tool call, possibly persisted ("Always allow Bash(git:*)"). Fires `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`. Two raise paths share the engine machinery. One is CC's MCP permission-prompt subprocess, consulted when CC would run `Edit`/`Write`/`Bash` on a path with no static allow rule. The other is the Codex app-server approval bridge (`item/commandExecution/requestApproval` / `item/fileChange/requestApproval` JSON-RPC requests, under `approvalPolicy: untrusted` inside the agent sandbox, else `on-request`).

Both render as a card the user must act on, but they are two separate event lanes. **A "permission prompt" is not a third event family**: it is whichever of these two raised it.

In practice:

- "Notify me when a coding agent is asking for permission" means one of two subscriptions. `UserQuestionAsked` covers AskUserQuestion and any permission-style prompt routed through the question registry. `CodingAgentPermissionRequest` covers true permission prompts. Pick by what the user wants to act on, or wire a trigger to each.
- `CodingAgentPermissionRequest` reaches a human card in every session. A catastrophic deny emits an already-answered pair instead. A trigger on this event, such as "When agent needs me", can announce it.
- Lucidos does NOT seed a default question-push trigger. Workspaces opt in by creating a user trigger (see `system-knowhow/triggers/actor.md` § "Worked example: push when agent needs me").

## The error gap

There is no `CodingAgentErrored`, `CodingAgentFailed`, or `CodingAgentCrashed` event. `rg -n "CodingAgent(Error|Errored|Failed|Crashed|Aborted|Canceled)" crates/lucidos-engine/src/` finds zero matches.

When a coding-agent session fails mid-turn (upstream API error, OOM-killed bash, empty assistant text on a non-cancel turn), `classify_result` (`agent_session/lifecycle.rs`) handles it. The engine:

- emits a chat-side `ResponseFailed { error }`, which sets the thread's projected `status = 'failed'` (the red error dot in the thread list);
- emits `CodingAgentIdled { has_changes: <whatever was on the branch> }`, so the dispatcher closes the turn and the UI leaves the "Working" state. This bookkeeping idle does NOT downgrade `failed` back to `idle`.

**A turn's verdict is sticky.** There are two. `failed` means the turn errored, or was interrupted with nobody coming back for it. `paused` means the user's own version switch interrupted it and the engine is resuming it.

Only a real *start* event clears one: `MessageReceived`, `CodingAgentUserMessageSent`, `PromptInjected`, `CodingAgentPromptSent`, `ContinuationRequested`. Every event that merely *closes out* the ended turn leaves the verdict alone, via the shared `preserving_verdict` guard (`engine/event_bus/mod.rs`). Those are the trailing activity stream, `ChangeProposed`, `CodingAgentIdled`, `SessionEnded` and `ResponseCanceled`. So the red dot (`failed`) or pause glyph (`paused`) stays until the user sends a follow-up or clicks Continue.

**`paused` is a promise of an auto-resume, and only that.** `AbortCause::status_sql()` splits on `AbortCause::promises_auto_resume()`, which reads the abort's ACTOR as well as its cause. A `ResponseAborted` carrying `EngineShutdown` **and** a device actor is the *Switch to new version* teardown. That is the one interruption the engine brings back by itself, and it settles the thread at `paused`. `StaleSettle` keeps the cancel-style idle mapping.

Everything else settles at the red `failed`, because nobody is coming back for it:

- `SafetyNet`, `ProcessKilled` and `AbortCause::SessionDropped`;
- a system-actor `EngineShutdown`, the shutdown fallback for a thread that started after the restart pre-emit;
- `RecoveryAfterRestart` in both roles: the boot sweep's crash boundary, and the boot floor *withdrawing* a promise it could not keep.

The frontend withholds the **Continue** button with the same predicate (`abortPromisesAutoResume`). So a paused thread never offers Continue, and a thread offering Continue never reads paused.

**A pending change does not change that answer.** The status never becomes `waiting` for one. The thread list ranks `paused` and `failed` above the changes dot by itself, and a `proposed` change state says a change is there. `waiting` is not a value `preserving_verdict` protects, so the dying subprocess's drain would overwrite it with `running`. It would also hide the thread from the boot floor, which selects on `status = 'paused'`.

**A provider block is a failure that never resumes.** When Codex ends a turn with a policy code (`misalignment_policy_violation`, `cyber_policy` or `bio_policy`), `ResponseFailed` carries `provider_error` and the card names the policy. The session is never resumed, because it would refuse again. The next message starts a new session from a *handoff brief* (`FreshSessionReason` `provider-blocked`), and the card offers a switch to Claude Code. A parent thread's card repeats OpenAI's words and says to rephrase the order or ask the user, never to resend it.

The guard matters most for a coding agent, because its subprocess outlives the terminal event. A *Lucidos Agent* turn's loop emits nothing after its own terminator. A coding-agent turn emits four more things.

An interruption (engine restart, watchdog kill) emits `ResponseAborted` while the agent is still alive. `external_terminal_emitted` then suppresses the duplicate *terminal* and drops the agent's further text, thoughts, tool calls and tool results. That includes any reply to the restart's interrupt, which Claude Code reports to the model as a user rejection. Only text streamed before the interruption can still land, as a `CodingAgentTextStreamed` milliseconds later.

Then come the `ProposalWithheld` for the work the agent leaves on its way out, `CodingAgentIdled` and `SessionEnded`. The guard keeps all four from walking the thread back from `failed` to `idle`. The drain lives in the shared `agent_session` layer, so Claude Code and Codex behave identically.

`ResponseFailed` and `CodingAgentIdled` are both triggerable. The cleanest wiring for "notify me when a coding agent errors" is:

```yaml
on:
  - event_type: ResponseFailed
    # Optional: scope to coding-agent threads. ResponseFailed payload itself
    # has no channel field; if you only want the coding-agent subset, fire a domain
    # event from the failure path instead (see option 3) or layer a separate
    # CodingAgentIdled subscription.
run:
  intent: "Send me a push notification that the response failed."
```

Or, if you specifically want "coding-agent turn ended without a clean response":

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      has_changes: false
      reason: "engine_restart_interrupt"
run:
  intent: "Tell me the coding agent needed engine recovery and is paused for me to restart."
```

For a workspace-defined failure name, emit a domain event:

```bash
lucidos events emit CodingAgentFailureObserved \
  --summary "Coding-agent turn failed" --payload '{"kind":"api-error"}'
```

The `events(action="emit")` LLM tool also writes a `SystemEvent::DomainEvent` that reaches
the matcher. Use this when the engine's ThreadEvents lack the discriminator
you need, such as OOM versus API-503.

## Notes on `CodingAgentPromptSent`

A user follow-up to a live session writes one too, as the run loop forwards it, with the user's text and no `origin`. Its `MessageReceived` is the user's message. Otherwise `CodingAgentPromptSent` marks prompts the engine synthesized:

- the merge-conflict explanation injected after `MergeConflictDetected`;
- the `MissingHardeningDetected` recovery prompt asking the coding agent to run `/harden`;
- the empty `CodingAgentPromptSent` right after `UserQuestionAnswered`, a "thinking" placeholder while the coding agent processes the answer. It is skipped for two answer kinds, since no turn of their own follows. On `Canceled`, the cancel-stamp path (`claude_code_stop`, `threads(action="archive")`) tears the agent down, so the marker would strand as an empty step. On `Superseded`, the replacing follow-up emits its own `CodingAgentPromptSent` moments later. See `emit_resume_marker_for_cc_answer` in `crates/lucidos-engine/src/engine/agent_question.rs`.

It carries `origin: Some(MessageOrigin::Engine { reason: ... })` on the wire. Workspaces rarely need to subscribe: it is an audit trail event, not a lifecycle signal.

