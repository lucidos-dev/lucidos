---
name: ThreadEvent Reference, Voice session (a thread being spoken to)
description: Part of ThreadEvent Reference. Its index lists the sections it covers.
---

One part of `system-knowhow/thread-events`, which lists every part.

## Voice session (a thread being spoken to)

The lifecycle of one *voice session*. Voice is a **mode of a chat thread**, never
a kind of thread (ADR 0148). So a session leaves the thread's `source` at `chat`,
opens no `channel` of its own, and moves neither status nor section: a live
microphone is not a turn.

Voice is **experimental and off by default**. None of these events can occur
until a workspace sets the `voice_enabled` preference. A session runs on the
*home thread* only (ADR 0362), so these events land on no other thread.

There is no voice-session table. The two lifecycle rows below *are* the session,
which is what lets the boot sweep find one whose engine died mid-call.

| Event | When it fires | Volume | Persisted | Triggerable |
|---|---|---|---|---|
| `VoiceSessionStarted` | A voice session opened on this thread. Carries `session_id: Uuid`; the `actor` (from `EventMeta`) is the device that opened the socket. One session may be live per thread, so a second upgrade is refused and writes no row. Placing a call bumps the thread's recency and nothing else. It does NOT promote a draft: connecting is not a conversation, so the first spoken word does that (ADR 0167). | lifecycle (rare per thread) | yes | yes |
| `VoiceSessionEnded` | The session closed. Carries the same `session_id`, `duration_secs: u64`, and `reason`. Reasons: `hangup` (the caller rang off); `agent_hangup` (the caller said they were done and Lucidos rang off, ending the call but never the work); `disconnected` (the socket died with no goodbye); `provider_failed` (the talker could not go on); `engine_shutdown` (the engine went away, or the boot sweep settled a start its process never ended). A sweep-settled row carries `duration_secs: 0`, since the engine holding the clock is gone. | lifecycle (rare per thread) | yes | yes |
| `SpokenReplyGenerated` | What the talker said in one finished turn of speech. Carries `session_id: Uuid`, `text: String`, `interrupted: bool` (the caller spoke over it, so only that much was heard) and an optional `spoken_secs_before: f64`. Written when the provider ends that turn, so `created` IS when the words stopped (ADR 0201). `spoken_secs_before` is how long the talker had been speaking by then. The transcript places the row at `created` minus it, where the talker BEGAN, before every step the words were said over (ADR 0206). It is absent on a reply that streamed no deltas, which reads at `created`. One per provider TURN, whether the talker composed the words or read the agent's answer aloud. A transcriber cuts sentences where the speaker breathes, so the transcript and the agent's history join neighbouring rows back together. A reply the caller CUT OFF carries only what they heard; the later full-turn report writes nothing (ADR 0200). A reply cut off before any word writes nothing. The `actor` names the talker as a guest agent, so the agent sees it under the talker's label, not as its own prior turn (ADR 0150). It is `Metadata`, because the agent's turn owns the thread's status. Like the spoken message, it MAKES THE THREAD REAL (ADR 0167), which matters because the talker usually greets first. | a few per call | yes | yes |
| `SpokenMessageReceived` | The caller said something on a call: EVERY caller utterance, whatever the talker does with it (ADR 0201). Carries `session_id: Uuid` and `text: String`; the `actor` is the caller's device. Written when the provider ends the caller's turn, so `created` is when they stopped speaking. It starts no agent turn, which is why it is not a `MessageReceived`: that Start event would claim a turn that never runs. The talker's `WorkDelegated` starts a delegated turn. `Metadata`, and it moves no section. It MAKES THE THREAD REAL (ADR 0167): a draft the call was placed from becomes an ordinary thread, its stored draft is cleared, and every device is told. The caller's FIRST spoken words become the thread's `first_message`. They never retitle Home, which only the user names (ADR 0362). | a few per call | yes | yes |
| `WorkDelegated` | The talker asked for the agent with its `delegate` voice tool. Carries `session_id: Uuid` and `reason: String`, the talker's few words on what the caller wants. It is empty when the talker composed none, as on any protocol whose delegation frame carries no words. The transcript then draws no row, rather than putting the caller's sentence in the talker's mouth (ADR 0200). The `actor` names the talker as a guest agent. **`Start`: this row begins a delegated call's turn** (ADR 0201), and the turn anchors on it. No `MessageReceived` is written beside it, since the caller's words have their own row. | a few per call | yes | yes |

No payload here carries audio, and audio is never persisted.

**The talker decides whether a spoken turn needs the agent.** It holds exactly
three tools and none of them acts (ADR 0170). `delegate` takes a short reason
and writes `WorkDelegated`. THAT row starts the agent's turn, through the same
single-flight admission a typed message uses (ADR 0201).

`answer` settles something waiting on the caller: a question card, or a
permission card in any of its three lanes. It hands back a choice id the engine
issued, so nothing matches a spoken word against a label. The resulting
`UserQuestionAnswered` or `*PermissionResolved` is the row the screen writes,
bar its actor. A `delegate` is refused while one of those is open, because the
agent is blocked inside it.

`hang_up` ends the CALL when the caller says they are done, never the work, so
a turn in flight keeps running. It writes `VoiceSessionEnded` with reason
`agent_hangup`.

Each utterance is recorded exactly once, so a call's transcript is the
thread's transcript: the caller's words, the agent's answer where there was
one, and the `SpokenReplyGenerated` rows for what was said aloud.

**Every row lands when its own turn ends** (ADR 0201). So nothing finished
lives only in engine memory, and the caller's words read above the reply to
them.

**The tool is an ask, not a wake.** The talker never knows whether an agent
turn is running. It calls `delegate` for every request needing the agent, even
while earlier work is going. Single-flight admission decides whether that
starts a turn or joins one.

**What the talker says is not what the doer wrote.** The written answer is
handed over to be SAID in the talker's own words, never read out. Past 400
characters the talker gives the headline and asks whether the caller wants the
detail. The full text reaches the talker either way, so a "yes, go on" is
answered from it, not invented. The thread keeps the full text.

A session's spend lands as one `ContextCaptured` per spoken reply, with
`purpose: "voice"`. So a cost rollup reads voice like every other model call.

## Which call each purpose names

Every `ContextCaptured` row says which model call it records. `turn` is an
agent's own round trip, and an absent `purpose` (every row from before the
field) means `turn`. The other eighteen are *auxiliary model calls*: ones the
engine makes for itself, and ones an app or script makes through the proxy.
**Every model call the engine makes carries a purpose** (ADR 0242). Only the
*model call service* can call a model, so a call that records nothing does
not compile.

**Every row lands on a thread.** A call made for a thread records there. A
call no thread caused records on the workspace's *home thread*, which every
workspace has.

An auxiliary row carries `producer: "auxiliary"`, no `tools`, a
`context_window` of 0 (a single-shot call has no budget) and one body-less
section sized to the request. It is recorded per ATTEMPT, so a resampled title
or retried extraction leaves several rows, since each spent tokens. The
transcript never renders one.

Each auxiliary model preference has one purpose (ADR 0107). A purpose the user
can point at a model owns its own `model_*` / `reasoning_*` pair. One that
cannot says why: a fixed backend, or the agent's own chat model.

| `purpose` | The call | Model |
|---|---|---|
| `turn` | An agent's own round trip. The only one with a section breakdown. | the thread's |
| `title` | The thread title, resampled up to twice. | `model_title` |
| `change_summary` | The one-line *change summary* for a change of several commits. | `model_change_summary` |
| `image_describe` | A caption for an image the user sent. | `model_image_description` |
| `memory` | Fact extraction over one message. | `model_memory` |
| `conversation_summary` | The paragraph standing in for a thread's older assistant turns. | `model_conversation_summary` |
| `summary_compaction` | One round of the compactor writing a summary line for the Tree memory module. Recorded on the thread the line summarizes, or on the thread that wrote an artifact it summarizes, else on the home thread. | `model_summary_compaction` |
| `memory_find` | One batch of the recall tool's `find`, judging about 40 summary lines against a query. Recorded on the thread that asked; a `find` from the API or CLI with no `--thread` records on the home thread. | `model_memory_find` |
| `query_classification` | The yes/no questions in front of memory retrieval. | `model_query_classification` |
| `read_decision` | The turn-end gate's forced *read decision*. It runs for a turn that left the decision open, including when the agent said yes during a trigger run or one of its wakes. Recorded on that thread. | `model_read_decision` |
| `image_gen` | One `generate_image` call. | `image_model` |
| `voice` | One spoken reply from a *voice session*'s talker. | `model_voice_talker` |
| `command_judge` | The *command guard*'s judge over one ambiguous command, on either backend. | `model_command_judge` |
| `judge_tool` | One call of the agent's own `judge` tool. | fixed: TypeSafe |
| `intent_loop` | Retired: one round of the removed intent sub-loop. Old rows still carry it, and nothing records it now. | the agent's own |
| `memory_correction` | The verdict behind `memory(action="correct")`, over which entries to delete. | the agent's own |
| `artifact_summary` | The summary written for a file the `import_file` tool imported. | the agent's own |
| `side_question` | One *side question*: a Lucidos Agent round, or a Claude Code session copy, or a Codex ephemeral fork's whole run. | the agent's own |
| `web_search` | One `web_search` call, on whichever backend answered. | fixed per backend |
| `proxy` | One model call an app or script made through the credentialed proxy (`/api/v1/proxy/...`, `lucidos proxy`). Recorded on the thread whose subprocess made it, else on the home thread. A call made during a trigger fire also carries that trigger in `emitting_trigger_id`, beside its thread when the fire has one. Only a reply carrying a usage block records. | the caller's |

`memory_correction`, `artifact_summary` and `side_question` run
on the agent's own chat model, as `intent_loop` did. They own no preference, so Settings offers no
row for them. `judge_tool` and `web_search` own none either, for a different
reason: their backend fixes the model. `proxy` owns none because the caller
names the model in its own request.

## Transient: never persisted, broadcast over SSE only

Transient names are past tense too. They cannot trigger, since the matcher sees only persisted events. They drive live UI state (streaming preview, in-app refreshes, navigation) and parent-thread fan-out signals. A request the user must answer is never transient: it is a persisted *form request* (§ Form requests), so a lost stream frame cannot lose it.

| Event | When it fires | Volume |
|---|---|---|
| `CumulativeTextUpdated` | One snapshot of the assistant's streaming buffer (cumulative text so far). Emitted at every flush boundary beside the persisted delta in `TextStreamed`; the frontend overwrites with the latest. Legacy alias: `TextStreaming`. | high-volume-streaming |
| `LlmCallRetried` | The chat agentic loop is retrying an LLM call (rate limit, transient API error, "retry with different approach" path). Carries `reason: String`. Legacy alias: `Retrying`. | per-action |
| `PreambleCompleted` | Reserved: defined on the enum and skipped by the projection's transient match arm, but **not emitted** by any production code path. Legacy alias: `PreambleCompleting`. | n/a |
| `PushNotificationRequested` | Request event that prompts the device to register for web push. Empty payload; its `actor` names the device whose push preference was set, and only that device asks. Legacy alias: `PushNotificationRequest`. | lifecycle |
| `AppUiRefreshRequested` | Tells an open app iframe with `app_id` to reload itself. From `refresh_app` it carries the turn's *last used device* as its `actor`, and only that page reloads. The end-of-turn refresh carries none, so every page with the app open reloads. Legacy alias: `RefreshAppUI`. | per-action |
| `AppUiCaptureRequested` | Asks an open app iframe to capture state for `request_id`. It carries the turn's *last used device* as its `actor`, as `NavigationRequested` does, and only that page answers. The reply lands via `POST /api/v1/app-capture`, which refuses an answer from another device with 409. With no last used device, there is no actor and any page may answer. `save_format` (`png`, `jpeg` or `webp`, absent by default) asks for a sharp picture to save as a file. Legacy alias: `CaptureAppUI`. | per-action |
| `WidgetFitRequested` | Asks one open Lucidos window to run a *widget fit check* on `app_id` for `request_id`. Normally its `actor` names the selected Device. With `renderer_session_id` and `renderer_device_id`, the actor instead names the verified source agent thread. Only the shell selecting that public handle and logical device renders. No event carries the renderer credential. The page renders each of `sizes` on both surfaces at `ui_scale` percent. `scale_device_id` selects the frames' appearance. The reply lands via `POST /api/v1/widgets/fit-report`; a wrong responder receives 409 without consuming the request. See `docs/widget-fit-renderer.md`. | per-action |
| `NavigationRequested` | Tells the frontend to navigate (URL, intra-app route, etc.). Carries `payload: String`. An agent navigate (`navigate_ui`) also carries an optional device `actor`: the device in the tool's `device` argument, else the turn's *last used device*. Every page drops the event unless the actor is its own device. So exactly one device acts: it opens the target if it shows the thread, else it shows a toast the user taps. The actor is absent for a turn with no device and for the SDK app-iframe (nil-thread) path. Only a connected page receives it; nothing stores, retries or confirms it. So the `navigate_ui` result says whether the target device had Lucidos visible in the last 2 minutes. If not, it offers `send_notification` with a navigate tap, which stays in the inbox. | per-action |
| `CodingAgentThreadSpawned` | A child coding-agent thread (spawned via `run_coding_agent` / `run_thread`) started. Carries `cc_thread_id`, `title`, `agent`. SSE-only: the child's persisted record is its own thread row. Alias: `CcThreadSpawned`. | per-action |
| `CodingAgentDiffChanged` | A coding-agent worktree post-commit hook found the branch's diff and moved the change state between `none` and `unproposed`. Carries `has_diff` (the git fact) and a full thread aggregate, so the frontend shows or hides the Diff button at once. Does **not** imply `ChangeProposed` / Apply readiness. | per-action |
| `ChildrenCountChanged` | A parent or ancestor thread's aggregate metadata changed. Carries the full updated aggregate (`active_children_count`, `total_children_count`, `blocking_descendant_count`, `attention_descendant_count`, …). Fires when (a) a direct child terminates and the parent's counts shift, or (b) any descendant's "blocking" or "attention-needing" predicate flips. Those predicates are Running, WaitingForUserAnswer, or `has_pending_changes` && CodingAgent (see `is_blocking` / `is_attention_needing`). In case (b) every ancestor on the chain gets its updated counts. Drives the "Active children" badge, the blocked Archive in the thread menu (`blocking_descendant_count`), the recount Archive runs on a thread already archived, and the Current-bubble routing in `display_section` (`attention_descendant_count`). | per-action |

## Indexable text

`ThreadEvent::indexable_text()` returns the text the memory store indexes. Only the types in `ThreadEvent::INDEXABLE_EVENT_TYPES` return any, and a memory rebuild loads only those rows. The types:

- `MessageReceived`.
- `SpokenMessageReceived`: what the caller said on a call.
- `PromptInjected`, only without `injected_message_id`. With one, it echoes a `MessageReceived` that memory already holds.
- `ResponseGenerated`, `ResponseCanceled` and `ResponseAborted`.
- `ChildThreadCompleted`: its `summary`.
- `ImageDescribed`: its `description`, the only text an image-only turn leaves.

Every other type returns `None`.

## Concrete payload shapes (selected)

`CodingAgentIdled`, `UserQuestionAsked`, `UserQuestionAnswered` and the `CodingAgentPermission*` pair are in `system-knowhow/coding-agent-events.md`. Below are the most-asked chat, lifecycle and change variants.

