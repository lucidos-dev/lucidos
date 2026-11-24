---
name: ThreadEvent Reference, Concrete payload shapes (selected) (continued)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Concrete payload shapes (selected) (continued)

### `MessageReceived`

```json
{
  "type": "MessageReceived",
  "data": {
    "text": "Summarize my open PRs.",
    "user_image_hashes": [],
    "device_id": "device-abc123",
    "parent_thread_id": null,
    "spawning_event_id": null,
    "mode": "human",
    "model": "claude-opus-4-7",
    "reasoning_effort": null,
    "provider": "anthropic",
    "origin": {
      "kind": "device",
      "device_id": "device-abc123"
    }
  }
}
```

`mode` is `ActorMode` (`human` / `agent` / `engine`). `origin` is the structured `MessageOrigin` (`Device` / `Api` / `Workspace` / `ThreadLink` / `Engine` / `System`). Old DB rows may lack `origin`; the frontend's `legacyOrigin()` synthesizes it from `device_id` / `parent_thread_id`.

Routed workspace spawns add `caller_gateway` and `routing_gateway` to the
workspace origin. The first identifies the gateway hosting the source thread;
the second identifies the gateway that routed the operation. Both come from
the verified operation, never caller-supplied HTTP identity headers.
Older workspace origins omit these optional fields.

**A device is stored by id only.** A `Device` origin or actor carries `device_id` and no name, and `MessageReceived` carries no `device` name. Lucidos shows the device's current name, so a rename reaches older events. Older rows carry a `label` (and `device`) that nothing reads. An app gets the id and no name: no SDK call lists devices today.

`voice_session_id` names the *voice session* a message was **spoken** on. No new `MessageReceived` carries one, since a caller's words are a `SpokenMessageReceived` (ADR 0201). Older rows still carry it, and the transcript reads it to mark such a bubble as spoken.

The `Api` variant carries an optional `source_thread_id`, and an optional `emitting_trigger_id`:

```json
"origin": {
  "kind": "api",
  "user_agent": "curl/8.7.1",
  "mode": "agent",
  "source_thread_id": "9c1f-..."
}
```

Set when the engine recognises a Lucidos-spawned subprocess (coding-agent session, `run_bash`, `run_python`, scheduled script, `lucidos` CLI). Detection uses the **thread-bound origin token**. Every spawned subprocess gets its own `LUCIDOS_AGENT_ORIGIN_TOKEN` in its env, shaped `<thread-id>@<depth>@<trigger>.<mac>` under a per-engine-startup HMAC secret. A `-` stands in for any field the spawn does not carry. The `lucidos` CLI forwards the token as the `x-lucidos-agent-origin-token` header on every engine call. The Python shim does the same for urllib and requests.

The MAC covers the whole prefix, so all three fields are authenticated, not claimed. A subprocess can present only the token it was handed. The depth is the event-trigger chain depth (ADR 0138), and the trigger is the fire this subprocess belongs to (ADR 0137). Mutating HTTP handlers (`api::changes::apply_change`, `revert_change`, `discard_change`, `chat_submit`, settings writes, …) then stamp `Api { mode: "agent", source_thread_id: <spawning thread> }`, whatever the body claims. So agent actions never appear as "You" cards.

A verified token outranks a `caller_workspace` body field too. Only this engine's subprocesses hold one, so the call comes from this workspace, and the token replaces the claimed caller thread. A scheduled script has no thread, so its origin carries `emitting_trigger_id` instead. The spawn gate reads both to find which trigger caused a coding-agent run (`system-knowhow/coding-agent-events/concrete-payload-shapes-2.md` § "Every session uses the workspace's grants").

A token that does not verify, including a valid one re-pointed at another thread, counts as no subprocess at all. It falls through to the unattributed-API-client path that external clients take. A script calling bare `curl` sends no header, so it takes that path too. The old `x-lucidos-source-thread-id` header was unverifiable; it is gone, and nothing reads one.

Cross-thread chat injection from a subprocess is refused with 403 at `chat_submit`. The target must be the caller's own thread, or a not-yet-existing thread whose declared parent is the caller. `api::chat::subprocess_chat_legitimate` has the full allow/deny matrix.

#### `mode` and `origin` are attribution, and an agent may not fabricate them

Together the two fields answer "who authored this turn", and the projection
acts on `mode`. `human` sets the thread's `initiator` to the user and bumps
`last_user_action`, the drawer's recency sort. So an agent-written
`mode: "human"` turn is a record the user cannot tell from their own.

**An agent must never post a message the engine would record as human.** The
one chat entry point, `POST /api/v1/chat/stream` (`api::chat::chat_submit`),
accepts `mode: "human"` from only two callers:

- a caller whose `device_id` resolves in the `devices` table (the user's own
  client, which sends `x-lucidos-device-id` on every mutating request);
- a `caller_workspace` (the cross-workspace contract, where the calling
  workspace vouches for its own human).

Everyone else is an *unattributed caller* and gets 403. See
`api::chat::human_mode_is_attributed`.

So dropping your origin token buys no privilege. A subprocess is held to
`subprocess_chat_legitimate` (which refuses `mode: Human` outright) for an
existing thread and for any `mode: "human"` post, `caller_workspace` or not. Only its `agent`
top-thread create skips that check: any caller may create a top-thread (ADR 0475). Its `engine` one
gets the 400 below, since the token says `agent`.

Three related refusals on the same path, all of which write nothing:

- **404** when `thread_id` names no existing thread and the request carries no
  create signal (`new_thread: true`, a `parent_thread_id`, or a
  `caller_workspace`). Otherwise a caller that reached the wrong engine would
  get its threads created there.
- **409** when the request asserted a different workspace than the answering
  engine serves (`x-lucidos-target-workspace`). The body names the actual
  workspace.
- **400** when the body `mode` contradicts the request's credential, e.g. a
  coding agent's origin token sent with `mode: "engine"`. The body names the
  credential's mode; send that one. See `api::chat::origin_agrees_with_mode`.

**A re-post of the same `event_id` runs once.** A client that lost the answer
to its POST may send the same body again. If the engine already accepted that
`event_id`, it answers `200` with the same `event_id` and starts nothing: no
second `MessageReceived`, no second turn. A refused request leaves its
`event_id` free, so a corrected retry runs. See
`api::chat::message_was_already_accepted`.

**A page's inputs record in the order it sent them.** The Lucidos client sends
`input_chain` (its page load) and `input_seq` (rising per thread) with every
message and answer. The engine admits each before it answers. It refuses one
that a later input from the same chain overtook, with **409** and
`"reason": "overtaken"`, and records nothing. A caller that sends neither field
is never refused this way. See `engine::chat::input_order` and ADR 0449.

If no tool covers what you were asked to do, say so. Never hand-roll HTTP to
the engine around it: see `system-knowhow/lucidos-cli/subcommands-7.md` § "Never post
to the engine API as the user".

The `ThreadLink` variant answers "who launched this thread", and it is **independent of `parent_thread_id`**:

```json
"origin": {
  "kind": "thread_link",
  "thread_id": "bc98-...",
  "spawning_event_id": "134e-...",
  "mode": "agent",
  "direction": "parent"
}
```

`parent_thread_id` is the *callback linkage*: it makes the launching thread resume when this one finishes, increments its `active_children_count`, and is what the `thread_summaries` projection stores. The origin is *display attribution*: who to name and link in the message route popover. A `relation: "child"` spawn carries both. Two shapes deliberately carry the origin with **no** linkage:

- a *top-thread* (`relation: "top"`), which names its *spawning thread* but reports back to nobody;
- a *child follow-up*, which attributes the message to the parent without re-counting an existing child.

Beside the linkage, a child's spawning message carries `wake_mode`: `no-wake`, `wake` or `wake-when-all-done`, the child's *wake mode* (ADR 0451). It is stored once, with the linkage, and lasts the child's life. Every other message omits it, and so does a spawn from before wake modes, whose turns fall back to the *wake decision*.

So never infer parent-ness from a `ThreadLink` origin; read `parent_thread_id`.

`image_description` on this payload is **deprecated** and survives only on legacy rows from before `ImageDescribed`. New emissions omit it (`Option<String>` with `skip_serializing_if = Option::is_none`). Read the description from `ImageDescribed`, joined by `source_event_id`. The startup backfill emits one `ImageDescribed` per legacy `(source, hash)` pair, so historical rows read the same way.

### `QueuedMessageRemoved`

```json
{
  "type": "QueuedMessageRemoved",
  "data": {
    "removed_message_id": "550e8400-e29b-41d4-a716-446655440000",
    "channel": "chat",
    "actor": {
      "kind": "device",
      "device_id": "device-abc123"
    }
  }
}
```

`removed_message_id` is the event id of the `MessageReceived` the user removed from the queued follow-up list. The original stays in the append-only log. It bumps no section, recency or message count. On a chat thread it settles a `running` status to `idle` when no turn has begun since the last terminal event. The removed message would have started the next one.

The frontend hides the matching exchange only while it has no steps.

A device chat Stop restores accepted text and images into durable compose atomically with this marker (ADR 0710). Its `compose` report carries the resulting revision and epoch, so response loss cannot lose the restored draft. Clients reconcile the report rather than appending again. The queued-message removal endpoint returns `{compose: report}` when Stop won a concurrent removal; Edit reconciles it too. Ordinary trash removals return `{compose: null}` and keep their existing behavior.

On a chat thread the marker is the *consumption claim*: a message is taken back or consumed, never both. The event bus decides it under the thread append lock (ADR 0706). It refuses this marker once the agent has the message: a `PromptInjected` naming it, a `ChatTurnSettingsResolved` anchored on it, or a `UserQuestionAnswered` with its id. `POST /api/v1/chat/queued-message/remove` then answers 409 with `reason: "already_read"`. Once the marker lands, the bus refuses each of those for the message instead. A message removed before it was recorded never gets a `MessageReceived`.

On a Claude Code thread the engine has already written the message to the agent, so the marker follows the agent's answer. The session asks Claude Code to drop it (`cancel_async_message`) and records this event only once Claude Code confirms. A message the agent already read gets no marker, and `POST /api/v1/chat/queued-message/remove` answers 409 with `reason: "already_read"`.

The marker outlives the engine. It is one of the three the *stranded queued message* predicate reads. So a retraction made while the engine was down still beats the next resume's recovery (ADR 0236).

### `ImageDescribed`

```json
{
  "type": "ImageDescribed",
  "data": {
    "source_event_id": "550e8400-e29b-41d4-a716-446655440000",
    "hash": "abcd1234...",
    "description": "A screenshot of a calendar invitation showing 'Standup' on March 17, 2026 at 09:00.",
    "model": "claude-haiku-4-5"
  }
}
```

`source_event_id` is the `MessageReceived` this description applies to. `hash` is the sha256 of one attached blob (one entry in `MessageReceived.user_image_hashes`). A multi-image message emits one event per hash, all with identical `description` text, so collapse on `source_event_id`. `model` is the Flash model that wrote the text (`claude-haiku-4-5`, `gemini-2.5-flash`, …). Rows from the one-shot startup backfill say `"backfill"`, since the original model was not recorded.

`ThreadEvent::indexable_text()` surfaces the `description`, so it is **indexed into memory** like a message. Otherwise a "what's this?" plus screenshot turn would leave no memory trace, since the typed text carries none of the image. Coding-agent threads never emit `ImageDescribed` (it runs only in the chat agentic loop), so their image turns index text-only.

### `ConversationSummarized`

```json
{
  "type": "ConversationSummarized",
  "data": {
    "summary": "Worked through the alignment pass on the edit timeline. The off-by-one on the splice boundary was fixed. Export presets were left as they are.",
    "covers_through_event_id": "550e8400-e29b-41d4-a716-446655440000",
    "covered_count": 31,
    "model": "gemini-3.8-flash"
  }
}
```

This event is the whole **cache** for the older-turn summary. There is no
table, so the paragraph survives an engine restart because the event does. The
first success holds: a later summariser failure reuses this paragraph instead
of rendering a bare "(N earlier messages not shown)" line.

`covers_through_event_id` addresses the newest assistant turn the paragraph
covers. `load_chat_history` compares it against the current older segment. It
re-summarises only when the assistant turns past that boundary exceed
`HISTORY_SUMMARY_REFRESH_AFTER`. Otherwise it reuses the paragraph and renders
the uncovered assistant turns compacted, so nothing is silently dropped.

**The refresh runs detached, so this event lands one turn late.** The turn that
finds a refresh owed renders from the cache as it stood. Awaiting the call
would delay that turn's first step for a paragraph only the next turn uses.

**User turns are never summarised** (ADR 0102). They render verbatim in the
older region, so a constraint stated 40 turns ago is still in the prompt word
for word.

**Every row covers its own thread** (ADR 0124). A chat turn reads only its own
events, so no other conversation's content lands under this thread.

### `ResponseGenerated`

```json
{
  "type": "ResponseGenerated",
  "data": {
    "text": "You have 3 open PRs: …",
    "images": [],
    "model": "claude-opus-4-7",
    "reasoning_effort": "medium"
  }
}
```

`text` is omitted on the wire when empty (`skip_serializing_if`). An empty
`ResponseGenerated` is a **benign empty completion**: a clean, model-decided
stop (`end_turn` / Gemini `STOP` / OpenAI `stop` / `completed`) with no text and
no tool calls. The thread completes Idle with no red error, and the frontend
shows a neutral note that the agent finished without writing a reply. The genuine failure shapes (truncation, safety block,
dropped output, unrecognised stop) still emit `ResponseFailed`. The split is
provider-agnostic: see `classify_empty_completion` / `normalize_finish_reason`
in `crates/lucidos-agents-api/src/engine/agentic_loop/helpers.rs`.

### `ResponseCanceled` / `ResponseAborted` / `ResponseFailed`

```json
{ "type": "ResponseCanceled", "data": { "text": "partial…", "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "user_stop" } }
{ "type": "ResponseAborted",  "data": { "text": "",        "images": [], "model": "claude-opus-4-7", "reasoning_effort": null, "cause": "engine_shutdown" } }
{ "type": "ResponseFailed",   "data": { "error": "upstream 503: model overloaded" } }
```

`cause` values:

<!-- generated:begin CancelCause -->
CancelCause wire values:

- `user_stop`
- `user_action`
- `superseded_by_followup`
- `effort_restart`
- `refused`
- `hold`
- `unknown`
<!-- generated:end -->

`user_stop` means the Cancel button; `user_action` means Apply / Discard / Archive on a running thread.
`unknown` covers legacy DB rows. `superseded_by_followup` means a follow-up interrupted a mid-turn **Codex** turn.
The engine ran the follow-up as the next turn, keeping partial work.

`effort_restart` appears on older threads only: a reasoning-effort pick interrupted a Claude Code turn.
A `ContinuationStarted` with reason `effort_restart` resumed it.
Nothing emits it now, because the rung locks while a turn runs (ADR 0542).

`hold` means a *hold* on this thread or an ancestor stopped the turn (ADR 0805). It leaves no Stop hold: Resume ends the hold it belongs to.

`refused` means the owner answered **Don't allow** on the card a coding-agent start waited on.
A caller Lucidos cannot verify started that run, so the run parked on the card with no terminator, and the thread read `waiting_for_user_answer`.
The refusal closes each start that waited, so a later Allow once carries none of them. It is not a failure, and the frontend labels it "Not allowed".

<!-- generated:begin AbortCause -->
AbortCause wire values:

- `engine_shutdown`
- `safety_net`
- `recovery_after_restart`
- `process_killed`
- `stale_settle`
- `session_dropped`
- `snapshot_restored`
- `unknown`
<!-- generated:end -->

`unknown` covers legacy rows. `session_dropped` means the run future was dropped because its caller was cancelled.
The session entry's drop-guard emits it.

On a *coding-agent thread*, `user_stop` is a **resumable turn boundary**, not a terminator. The `Cancel` button uses the backend's native interrupt, so the session stays alive. `CodingAgentIdled` follows (with the `cc_session_id` when available), the branch is kept, and the next message resumes the conversation. `user_action` (Apply / Discard / Archive) DOES terminate, via its own lifecycle event. See `system-knowhow/coding-agent-events/billing-evidence.md` § `CodingAgentIdled` "Cancel = Esc".

`superseded_by_followup` is mechanically a cancel, but the user **steered** rather than stopped. There is no `ResponseGenerated` and no proposal for the partial work. The branch is kept, and the follow-up turn's proposal includes both turns' files. The frontend renders it **neutrally**: a plain "Done", with no "Canceled ✕" badge and no "Response canceled" panel. Only Codex produces it: Claude Code steers a mid-turn follow-up via stdin, and idle-Codex follow-ups use `turn/start`. See `docs/plans/2026-06-21-codex-followup-redirect-label.md`.

`effort_restart` is the same kind of redirect, with the same rendering, branch and parent rules.

A redirect cancel does not change the thread's status. Both redirect causes keep it `running`, and so do the `CodingAgentIdled` and `SessionEnded` that close out the interrupted run, until the next start event. The status is never `idle` in the gap before the redirected turn starts. The thread keeps its Stop button and no "cache warm" line shows. A `user_stop` or `user_action` cancel settles the thread as before.

