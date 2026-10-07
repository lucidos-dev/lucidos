---
name: Coding Agent Lifecycle Events
description: ThreadEvents a Claude Code or Codex session emits, which ones the scheduler blocks, and how to spawn one with `run_coding_agent`. Load for "notify me when a coding agent finishes / asks a question / errors", "which folder should the coding agent edit", "wait until the agent is idle", "watch for a permission prompt", "CC session", "AskUserQuestion".
---

# Coding Agent Lifecycle Events

`CodingAgent*` is the engine's umbrella name for events a coding-agent session emits, Claude Code or Codex. Both backends translate their CLI output into the same `ThreadEvent` enum. A `coding_agent: CodingAgent` field tells them apart. It defaults to `ClaudeCode` on legacy DB rows, and `#[serde(alias = "agent")]` decodes rows persisted before the rename.

The session's *worktree* wraps a git repo that depends on `coding_agent_kind`:

- `lucidos` (default): a full checkout of the Lucidos source repo. It exists only when the engine was launched from a Lucidos source checkout. A packaged install ships the binary alone, so the compose destination picker hides the "Lucidos source" target and `run_coding_agent` with `folder` omitted is refused.
- `app`: a sparse checkout of the workspace git, narrowed to `data/apps/<id>/` (see *app worktree*). Apply ff-merges to the workspace git's local `main`, with no engine restart and no `/harden`. `AppUiRefreshRequested` emits when iframe-bundled files change.
- `external`: a full checkout of the user-registered external git repository. No Apply / Discard surface.

`coding_agent_kind` ships on every `SessionStarted` event and persists in `thread_summaries.coding_agent_kind`, so the apply path dispatches without re-reading the event log.

### Choosing `folder` when you spawn one

`run_coding_agent`'s `folder` argument selects the kind above, so pick it before
you call. Ambiguous? Ask which folder first.

| `folder` | Kind | Notes |
|---|---|---|
| omitted | `lucidos` | Edits the Lucidos platform's own source. Available ONLY on an install whose engine was launched from a Lucidos source checkout; the chat system prompt's "WHAT A CODING AGENT CAN EDIT ON THIS INSTALL" section states which install this is. Full `/harden`; Apply may need an engine restart. |
| `data/apps/<id>` (workspace-relative), or an absolute app-folder path | `app` | Whole app folders only. For a one-line edit prefer the chat path (file tools plus the `lucidos` CLI) over a whole session. |
| a registered repository name or UUID from `manage_repositories` | `external` | Register the repo first; an unregistered git folder is refused. |

Refused, each with an error rather than a silent fallback: an unregistered git
folder, a non-git directory, any `data/` path outside `data/apps/<id>/`, a
subpath inside an app, a bare file path, the whole of `data/`,
`<workspace>/.lucidos/`, and system paths.

**Spawning returns immediately, and the spawn ack is not a result.** Read the
child's final response text for pass or fail before you act on it or report it.

**Do not restate review, merge or hardening behavior in the brief.** The
`folder` kind sets them, as the table shows. A restated one reads as a
prohibition: a stray "do not harden" has made a session skip a required step.
Mention one only to ask for something other than its default.

**Cross-workspace.** Set `workspace` to the target workspace's basename, and the
tool POSTs to that engine, where the session lands. It requires
`relation: "top"`, because a child auto-resume callback does not cross
workspaces. Child plus cross-workspace is refused with an error. `folder` then
resolves on the TARGET workspace, so the app must be installed or the repo
registered *there*. That engine applies its own source-checkout check.

**`workspace` reaches only workspaces of the same install.** It resolves the
basename beside the caller's own workspace. No workspace a packaged install
serves has a source checkout, so Lucidos platform work from there goes to
another install. Use `run_bash` with the CLI, which takes an absolute path.
Pass `--coding-agent codex` for a Codex session, and `--image <path>` to carry
an image the user attached:

```bash
lucidos spawn-thread --to <absolute workspace path> --relation top \
  --coding-agent claude-code --message '<task>'
```

The refusal above is about THIS install, not the caller, so this route stays
open. The `lucidos spawn-thread` section of `system-knowhow/lucidos-cli.md` has
the full flag list.

Rebuilds, restarts and live-build checks after Apply: see the chat system
prompt's ENGINE RESTARTS and APPLYING & VERIFYING CHANGES sections.

**Backend selection (Claude Code vs Codex).** The thread's FIRST send picks the backend: the compose destination picker's coding-agent chip sets `coding_agent` on the chat request. The default is `claude-code`, remembered per workspace via the `coding_agent_default` preference. The value ships on `SessionStarted.coding_agent` and persists in `thread_summaries.coding_agent`. It is **locked**: follow-ups and recovery always resume on the stored backend, since the other backend has no session to resume. A follow-up requesting a different backend gets `409 Conflict`.

Differences a workspace can observe:

- Codex emits coarse tool events (`command_execution`, `file_change`, `mcp_tool_call`, `web_search`, `todo_list`) instead of CC's named tools (`Bash`, `Read`, `Edit`, …). Trigger conditions on `CodingAgentToolCalled.name` must match per backend.
- `UserQuestionAsked` / `UserQuestionAnswered` fire for **both** backends. CC routes through its `AskUserQuestion` PreToolUse hook. Codex calls the `ask_user_question` tool on the `lucidos` MCP server (one question per call), which hits the same blocking engine endpoint. The answer returns inside the same Codex turn as the MCP tool result. Neither backend emits a `CodingAgentToolCalled` for the question tool (`AskUserQuestion` and `mcp__lucidos__ask_user_question` are suppressed), so wire triggers to `UserQuestionAsked`, not to the tool name.
- `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved` always fire for CC. They fire for Codex under the default `app-server` protocol (`approvalPolicy: on-request`). There, sandbox-escaping commands and out-of-worktree file changes raise the same PermissionCard, with `command_execution` / `file_change` as the `tool_name`. Under the `LUCIDOS_CODEX_PROTOCOL=exec` escape hatch, Codex runs with the OS sandbox (`--sandbox workspace-write`) as the only guard and emits **no** permission events.
- `/harden`, Apply / Discard, worktrees, branches and every event in this file are backend-agnostic: same lifecycle, same payload shapes.

Two recurring confusions: there is no `CodingAgentPermission*Event` separate from `UserQuestionAsked`, and there is no `CodingAgentErrored` event at all.

For the **full ThreadEvent enum**, with persistence and triggerability for every variant, see `system-knowhow/thread-events.md`. This file is the coding-agent deep-dive.

For trigger config syntax (cron vs the `on` subscription list, per-entry `condition` filters, `run.intent` discipline), see `system-knowhow/triggers.md`. For event-store column shape and the chat-side terminator events (`ResponseGenerated` / `ResponseFailed` / …), see `.claude/rules/db.md`.

## Triggerability: blocklist semantics

The scheduler forwards persisted ThreadEvents to the trigger matcher, unless they sit on a small **blocklist**. That is `core::event_subscription::is_subscribable`, called at the `BusEvent::Thread` arm of the scheduler subscriber in `crates/lucidos-engine/src/scheduler/mod.rs`. It drops the per-token streaming variants (`ThreadEvent::is_per_token_streaming`) and the side-question events (`ThreadEvent::is_side_question_event`). Two coding-agent streaming entries sit on it, `CodingAgentTextStreamed` and `CodingAgentThoughtStreamed`, beside the four side-question events.

So, for each entry in a trigger's `on` list (full shape in `system-knowhow/triggers.md`):

- `event_type: UserQuestionAsked`: works. Have the trigger call `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` to deep-link the push straight to the question. Take `event_id` from the trigger's `Source event id:` line.
- `event_type: CodingAgentIdled`: **works**. Add `condition: { has_changes: true }` to scope to "the coding agent finished and left work to review."
- `event_type: CodingAgentPermissionRequest`: **works**. Reacts to "the coding agent is asking permission for a tool call."
- `event_type: CodingAgentToolCalled` / `CodingAgentToolResult` / `CodingAgentPromptSent`: **works**, but these are per-action and chatty. Always add a `condition:` (e.g. `name: "Bash"`) or the trigger fires many times per turn. A condition key is a field path, so the command inside `args` is filterable too: `{ "args.command": { "$regex": "cargo test" } }`.
- `event_type: CodingAgentTextStreamed` / `CodingAgentThoughtStreamed`: does not fire. The scheduler blocks per-token streaming, so subscribing to either is a no-op.
- `event_type: SideQuestionAsked` (or any side-question event): refused. No agent, trigger or event wait may see a side question (ADR 0320).
- `event_type: <any chat-side lifecycle event>` (`ResponseGenerated`, `ResponseFailed`, `ChangeApplied`, …): works, same blocklist. See `system-knowhow/thread-events.md` for the full set.

The blocklist is not the only gate. A trigger is never woken by an event its own fire emitted, so an *intent* trigger subscribed to `ResponseGenerated` does not see its own. But a coding-agent session the fire STARTS runs on its own thread, so its `CodingAgentIdled` still wakes the trigger. That is what makes "wait for the session I started" work.

Across triggers, `max_event_trigger_depth` (default 5) bounds the chain, and that gate DOES follow the handed-off session. Every event the session emits carries the fire's own depth. So the waiting trigger fires at hop 1, while a chain that keeps going ends at the ceiling. See `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist" for both gates, and `system-knowhow/triggers.md` for what they mean when you author a subscription.

## The full enumerated list

All variants below are defined on `ThreadEvent` in `crates/lucidos-engine/src/engine/thread_events/event.rs`. Each has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename name: write new code (and new triggers) against the `CodingAgent*` form.

### Persisted, low-volume: terminal-state or one-per-turn

| Event | When it fires | Volume |
|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. One per user-typed message on a coding-agent thread. | One per user message |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries an `origin: Option<MessageOrigin>` so the route popover can render "Engine · …". Persisted for audit; not rendered as a chat bubble. | One per engine-driven injection |
| `CodingAgentInputRead` | The coding agent took in an input the engine forwarded. Claude Code reports it by replaying the input; Codex when its turn for the input starts. Carries `input_event_id`, the event that carried the input (the `MessageReceived` for a message, the `ChildThreadCompleted` for a child wake). Until it lands the input is owed, so the session keeps its subprocess at idle (ADR 0268). A message the user takes back before the read (a `QueuedMessageRemoved` on a Claude Code thread, ADR 0323) never gets one. The transcript marks the message "Sent", then "Read". `started_turn: true` (omitted when false) means the read opened a new turn after the last one ended, such as a queued message after a Stop. That read sets the thread running again and ends a *stopped child*. | One per forwarded input the user did not take back |
| `CodingAgentSettingsChanged` | Two roles. (1) The user changed model or reasoning effort mid-session via the in-thread control. The permission mode is not an in-thread control: it is the *coding-agent permission mode* preference, fixed at spawn. (2) Emitted once at backend init with `cc_session_id: Some(..)`, `claude_config_dir` and `claude_config_dir_explicit`, so all three are durable *before* the first `CodingAgentIdled`. Persisted so settings survive idle exit + respawn, and so a mid-turn engine restart can still resume: `lookup_latest_cc_session_id` reads `cc_session_id` here as well as from `CodingAgentIdled`. `claude_config_dir` is where transcripts live: `$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl`. `claude_config_dir_explicit: false` means `CLAUDE_CONFIG_DIR` was unset, so the dir is Claude Code's default `~/.claude`. Claude Code treats those as two logins, with different keychain entries and `.claude.json` files. A coding-agent thread is **pinned to the profile of its first session**. The engine replays that earliest pair on *every* later spawn (`lookup_pinned_cc_config_dir`): the same path when set, no variable when unset. It also scopes the auto-detected resume session id to that dir (`lookup_latest_cc_session_id_for_config_dir`). So only a thread's first turn adopts the live global toggle. | Once at init + on each user toggle |
| `CodingAgentPermissionRequest` | The coding agent asked to confirm a tool call. Claude Code raises this through its MCP permission-prompt subprocess (Edit/Write/Bash on a path outside the session's *working directories*, anything under `.claude/` or `.git/`); Codex raises it through the app-server approval bridge for sandbox-escaping commands and out-of-worktree file changes. The user resolves it via `POST /api/v1/permission/<request_id>/{allow,deny}`. **Clickable only on interactive (human-rooted) sessions.** An unattended trigger-rooted session auto-resolves: an auto-allow emits nothing, and an auto-DENY emits this event plus its resolution (see "Unattended auto-resolution" below). | Per tool call needing consent, plus one per unattended auto-deny |
| `CodingAgentPermissionResolved` | The above request was answered (or auto-resolved by recovery / supersession). Carries `allowed: bool`, an optional `persist_scope` (`narrow`/`broad`/`session`) recording which "Always allow"-style scope the user picked, and a `reason` for failure / orphan-recovery / superseded cases. | Pairs 1:1 with `CodingAgentPermissionRequest`. Three engine paths emit `allowed: false`: orphan-recovery (the agent died first), supersession (the user replied instead of clicking), and an unattended auto-deny. Each carries its own `reason`. |
| `CodingAgentIdled` | **The turn-boundary marker.** Emitted at the end of every coding-agent turn whose Result wasn't an engine-shutdown abort. Full payload under "Concrete payload shapes" below. | One per turn (a session normally has 1–3 across its life; many more if the user keeps replying) |
| `MissingHardeningDetected` | A coding-agent session ended without running the required `/harden`, so the engine auto-spawned a recovery hardening session. **Not a session terminator**: the thread stays active until hardening finishes. | Rare; only on the recovery path |

### Persisted, high-volume: pair with `condition:` (or, for streaming, blocked entirely)

The scheduler **blocks** the two streaming events. `CodingAgentToolCalled` / `CodingAgentToolResult` fire a few to a few dozen times per turn and reach the matcher. A trigger without a `condition:` fires on every tool call, so always scope by a field, e.g. `name: "Bash"`.

| Event | When it fires | Triggerable |
|---|---|---|
| `CodingAgentTextStreamed` | Each `text` chunk the coding agent streams to the user, one per assistant-message line or paragraph. Carries what the MODEL wrote, never the backend's own API-error banner. Claude Code reports an upstream drop as a `<synthetic>` assistant message flagged `is_api_error_message`. The engine skips it, because the same string returns as the turn's failure reason via `ResponseFailed`. Text a Claude Code sub-agent wrote carries `parent_tool_use_id`, the id of the `Agent` call that spawned it. That text is the sub-agent's narration, never the reply, so filter on the field's absence for the agent's own words. | **no (blocked: per-token streaming)** |
| `CodingAgentThoughtStreamed` | Each chunk of streamed reasoning the coding agent produces before its visible output. CC sends a `stream_event` `thinking_delta` (text on `delta.thinking`). The persisted CC JSONL keeps only an encrypted signature, so the live stream is the only source. Codex sends `item/reasoning/summaryTextDelta` / `textDelta` (app-server) or a `reasoning` item (exec). Coalesced into a few rows per turn and rendered as the live "Thinking" step. **Live on Codex, dormant on CC.** Codex: both drivers set `model_reasoning_summary=detailed`, so Codex threads stream reasoning *summaries* here. Codex's default summary mode emits no reasoning notifications at all (verified on codex-cli 0.142.5). CC: **dormant for current models, on every provider.** Anthropic's `thinking.display` defaults to `omitted` on every current model (Fable 5.1 and 5, Opus 4.7 through 5.5, Sonnet 5 and 5.5). So the `thinking_delta` carries empty text and this event does not fire. That holds on **both** Vertex and the first-party Anthropic API, even with `--thinking-display summarized` forced. It is an upstream Claude Code limitation in its headless `stream-json` path, and the raw chain of thought never returns in any mode. Switching CC's provider does **not** fix it. See `docs/temporary-measures.md` § `cc-reasoning-dormant`. **Progress notes are separate.** Opus 5.5, Sonnet 5.5 and Fable 5.x write a short note between tool calls. The engine's Vertex relay asks for them, so they arrive as `CodingAgentTextStreamed`, never as this event. | **no (blocked: per-token streaming)** |
| `CodingAgentToolCalled` | Each tool invocation the coding agent makes. Carries `name`, `args` (full JSON, stripped from what a client reads), optional `description`, and `tool_use_id`. The id pairs the matching `ToolResult` even when a permission prompt splits them across exchanges. A call a Claude Code sub-agent made also carries `parent_tool_use_id`, the id of the `Agent` call that spawned it; the transcript folds those steps under that call. A Claude Code call also carries `api_call_id`. Every tool call of one model API call shares it with that call's `ContextCaptured`, so each row shows that call's context figure. | yes (use condition) |
| `CodingAgentToolResult` | The result returned to the coding agent for a prior `ToolCalled`. Carries the same `tool_use_id` and `parent_tool_use_id`, and the call's tool as `name`. `result` holds the agent's whole output. See below. | yes (use condition) |

**What `CodingAgentToolResult` stores.** The agent runs each tool in its own process. Lucidos parses a copy of the agent's output stream to draw the steps, and this event records that copy.

- **`result` is the whole output.** NUL bytes are removed and Postgres passwords are masked, as in the call's `args`. So a `result` condition matches anywhere in the output.
- **Older rows hold at most 200 chars.** The writer used to cut every result to its first 200 chars. `GET /api/v1/events/:event_id/tool-result` serves an exactly-200-char result with a note that it may be cut short. The stored row is not rewritten.
- **`name` is the call's tool name**, such as `Bash`. It is empty on older rows, and on a result whose call the session never saw.
- **Neither the snapshot nor the live stream carries `result`.** Both stamp `result_stripped: true`, and the step detail fetches the text by event id. An SDK `lucidos.sse` listener sees the stripped shape too.

The call's `args` get the same treatment. The snapshot and the live stream drop them and stamp `args_stripped: true`, so an SDK listener sees no `args` either. Fetch them from `GET /api/v1/events/:event_id/tool-args`. The stored row keeps them, so an `args.command` condition still matches.

### Transient: never persisted, broadcast over SSE only

| Event | When it fires |
|---|---|
| `CodingAgentThreadSpawned` | A child coding-agent thread (spawned via `run_coding_agent` / `run_thread`) has started. Carries the new `cc_thread_id` + `title`. SSE-only: the child's own thread row is its persisted record. |

### `UserQuestion*`: the question / permission channel

These are NOT prefixed `CodingAgent*`, because the same machinery serves any agent that asks the user a structured question. Three raise paths exist: CC's built-in `AskUserQuestion` tool, Codex's `ask_user_question` tool on the `lucidos` MCP server (one question per call), and the chat agent's `ask_user_question` LLM tool. All three emit `UserQuestionAsked` and take their answer through `POST /api/v1/threads/{thread_id}/answer-question`. The engine branches on `meta.channel` for the resume side-effects:

- Coding-agent channel: a resume marker, plus a `ContinuationRequested` respawn if the subprocess is gone.
- Chat: wake the in-process tool waiting on the question wait registry.

| Event | When it fires |
|---|---|
| `UserQuestionAsked` | One of the three tools raised an interactive question. `meta.channel` records the lane: `claude_code` (the coding-agent channel, both backends) or `chat`. The raising agent blocks while the card is on screen. CC's PreToolUse hook and Codex's MCP server both long-poll the engine's internal endpoint, and the chat tool blocks in-process. `POST /api/v1/threads/{thread_id}/answer-question` emits `UserQuestionAnswered` and dispatches the channel's resume path. |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. `meta.channel` tells chat answers from coding-agent answers without looking up the `Asked`. |

## Concrete payload shapes

### `CodingAgentIdled`

```json
{
  "type": "CodingAgentIdled",
  "data": {
    "has_changes": true,
    "is_external_repo": false,
    "requires_restart": false,
    "cc_session_id": "abc123-…",
    "coding_agent": "claude-code",
    "worktree_path": "/Users/.../.lucidos/worktrees/thread-1a2b3c4d",
    "worktree_head_sha": "f6ae7364e…",
    "bg_bash_pending": false
  }
}
```

All fields except `coding_agent` are `#[serde(skip_serializing_if = ...)]`-gated, so they are missing from the wire at their zero value (`false` for bools, `None` for `Option`s). Read defensively: `payload.has_changes ?? false`, `payload.worktree_path ?? null`.

**A `condition` on this event can also name `thread_id`, which is NOT in the payload above.** The engine supplies it for every thread event, so `{ "thread_id": "<uuid>" }` scopes a wait or a trigger to one coding-agent session. It exists only at matching time. The event row stores the thread in the `events.thread_id` column, so a persisted payload does not hold it.

| Field | Type | When present |
|---|---|---|
| `has_changes` | `bool` | `true` iff the coding-agent branch has a non-empty net diff against its **diff base**, after filtering out runtime-only paths (`.lucidos/**` etc.; see `branch_changed_files` + `files_require_restart`). The diff base is the one the Diff button renders against (`default_diff_base`): `origin/<default>` when the local default branch has diverged, otherwise the local default. One function (`branch_changed_files`) gates the button and computes the diff, so the button never lights up on an empty diff. Carries forward from a prior idle when the turn made no new commits but the branch holds prior work. Drives the `coding_agent_has_diff` projection column (the WaitingBanner Diff button). During a live turn, the worktree post-commit hook can update that column earlier through `CodingAgentDiffChanged`. Only the aggregate `ChangeProposed` (non-empty `change_id`) sets `coding_agent_proposed`. It follows only when the turn ended `Generated` and `may_touch_change_state_at_idle` permits. Aborts, cancels and mid-turn deaths never create an Apply proposal (see the "Changes" section of `thread-events.md`). |
| `is_external_repo` | `bool` | `true` for sessions on a repo imported via `RepositoryImported`, not the Lucidos engine repo. Authoritative for the `coding_agent_is_external_repo` column the WaitingBanner reads to swap Apply for Done/Archive. |
| `requires_restart` | `bool` | `true` iff at least one file in the same filtered list matches `files_require_restart` (Rust source, `Cargo.lock`, certain bundled assets). Informational only: `ChangeProposed.requires_restart` is authoritative for the `coding_agent_requires_restart` column. |
| `cc_session_id` | `Option<String>` | The CC CLI session id at idle. `None` for recovery-emitted idles with no live subprocess (the "no-branch" and "stuck-session" recovery paths). `CodingAgentSettingsChanged` also pins the id at `Init`, so a turn an engine restart interrupts before its first idle stays resumable. `lookup_latest_cc_session_id` reads the newest non-null id across both event types. |
| `coding_agent` | `CodingAgent` | `"claude-code"` or `"codex"` (kebab-case wire values). Defaults to `"claude-code"` on legacy DB rows. `#[serde(alias = "agent")]` decodes rows persisted under the older `agent` field name. |
| `reason` | `Option<String>` | **Usually absent.** Only recovery stamps it: `"engine_restart_interrupt"` when a mid-turn-crashed session shows as "interrupted, click to continue" instead of auto-spawning. The frontend reads it to render the continue affordance. |
| `worktree_path` | `Option<String>` | Absolute path of the worktree the agent ran in, set by `run_session.rs` for normal turns. **`None`** on legacy rows, and when the worktree was `worktree remove --force`'d before the idle fired (the "stale session" cleanup path, the no-branch recovery path). |
| `worktree_head_sha` | `Option<String>` | `git rev-parse HEAD` in the worktree at idle time. The next spawn uses it to detect external user edits between turns. `None` on legacy rows, with no worktree, or when `git rev-parse` fails (e.g. a zero-commit branch). |
| `bg_bash_pending` | `bool` | **Recorded history only: it gates no proposal and drives no UI.** `true` iff the turn idled while a background task this thread started (a coding agent's `lucidos background-task run`) was still running and nobody had asked to stop it. A task the thread already stopped re-opens nothing (ADR 0369), so it does not count. It keeps nothing alive: the task's event wait re-opens the thread. `may_touch_change_state_at_idle` ignores background bash, so the change proposes the instant the coding agent idles. Harden-at-apply covers correctness: an un-hardened change re-runs `/harden`, tests included, before it can merge. **Startup recovery:** `propose_held_back_changes_on_startup` re-proposes at boot any idle coding-agent thread with a committed diff but no proposed change. It emits `ChangeProposed` with `origin: Engine{ reason: StaleSession }`. It skips a thread that is already proposed, has no diff, is external-repo, or has lost its branch. It also skips one with no authored commits left, where only an engine back-merge of main remains. That three-dot diff would re-surface already-applied files as a phantom change. |

**Fires on:** every coding-agent turn that ended on a `Result` other than engine-shutdown abort. That covers natural completion, `Failed` (the coding agent errored, OOM, empty assistant text), and the user's `Cancel`, which is a turn boundary, not a terminator. Engine shutdown does NOT emit `CodingAgentIdled`: recovery resumes the session on next start.

**Cancel = Esc (resumable):** the `Cancel` button is a real *interrupt*, not a kill. `POST /api/v1/claude-code/stop` (default, `StopReason::UserStop`) routes through `interrupt_agent`, which forwards CC's native interrupt (pressing `Esc` in the CLI). CC winds down the turn and emits a `Result`. The engine emits `ResponseCanceled(UserStop)` **+** `CodingAgentIdled` carrying the `cc_session_id`. The branch is **kept** even with zero commits (`SessionEndAction::KeepCanceledBranch` in `finalize`), so the next message `--resume`s the *same* conversation on the *same* branch. Apply / Discard / Archive still hard-stop via `stop_agent`, each with its own terminator.

A bounded fallback escalates to the hard stop if CC ignores the interrupt for ~8s. Causes include a hung socket, or a control request ignored during a long tool. The watchdog skips while a tool is in flight. Even then the turn is stamped `Canceled(UserStop)`, so the branch is kept and the session stays best-effort resumable.

### Side questions are recorded, and hidden from every agent

A *side question* is asked from side-question mode. A long press on the composer row's end button, or ⌥↵, turns the mode on, and its round Ask button asks the box. It is answered beside any running turn, from the thread's own context, with no tools. It may carry images. It works in Claude Code and Lucidos Agent threads, and each takes one path, text and images alike:

- **Claude Code** asks a copy of the session: the session's own command resumed with `--no-session-persistence`, under a settings file whose PreToolUse hook refuses every tool. It keeps the session's tools and appended system prompt, so the prompt cache holds, and it writes nothing to the transcript. Its cost is recorded as `ContextCaptured` with purpose `side_question`. Claude Code's own `side_question` control request is not used, since it carries text only (ADR 0324).
- **Lucidos Agent** makes one model call with a turn's system prompt, tools and history. A tool call is refused, never run. Its cost is recorded as `ContextCaptured` with purpose `side_question`.

The endpoints:

- **Ask:** `POST /api/v1/side-questions` with `{thread_id, side_question_id, question, image_hashes?}` returns `{answer}`. The client names `side_question_id` (a UUID), and `image_hashes` names blobs already uploaded to this workspace. A 400 refuses a draft, a Codex thread, a Claude Code thread with no session yet, an empty question, or an unknown image. A 409 refuses an id still running or answered, while a failed one may be asked again (a card's Retry). No refusal records anything. An ask waits at most 120 seconds.
- **Dismiss:** `POST /api/v1/side-questions/dismiss` with `{thread_id, side_question_id}` returns `{ok: true}`, or 404 when no such ask exists on the thread.
- **Recorded:** `SideQuestionAsked` (with any `image_hashes`), then `SideQuestionAnswered` or `SideQuestionFailed`, and `SideQuestionDismissed`. So a card survives reload and shows on every device. Startup fails every ask a restart left unanswered.
- **Hidden:** no agent ever reads them. `query_events`, triggers, event waits and every context builder leave them out, and they move no thread state.

A typed `/btw` is ordinary text. The chat route sends it as a normal message.

### `UserQuestionAsked`

```json
{
  "type": "UserQuestionAsked",
  "data": {
    "tool_use_id": "toolu_…",
    "cc_session_id": "abc123-…",
    "question": "Which approach should I take?",
    "options": [
      { "id": "opt-0", "label": "Approach A", "description": "…", "preview": "![A](artifacts/approach-a.png)" },
      { "id": "opt-1", "label": "Approach B" }
    ],
    "worktree_path": "/Users/.../worktrees/thread-…",
    "multi_select": false
  }
}
```

| Field | Type | Notes |
|---|---|---|
| `tool_use_id` | `String` | The agent's identifier for this question: the unique key in DB and in `UserQuestionAnswered.tool_use_id`. Per-question sub-ids take the form `{outer}#q{i}`, so a multi-question batch from one tool call gets one DB row per card. |
| `cc_session_id` | `String` | The CC session id at intercept, which `--resume` pins to. **Empty string** for questions from the chat agent's `ask_user_question` tool (chat is in-process, with no subprocess to resume). |
| `question` | `String` | The prompt text shown to the user. **For permission requests, this is the human-readable rendering of the tool-use the coding agent asked permission for** (see "Question vs. permission" below). Filled strictly from the tool input's `question` field, never from the optional `header` chip-label. Tool-call schemas are advisory, so the model can omit a `required` field. The engine therefore rejects a batch with any entry missing a non-empty `question`, before any `UserQuestionAsked` is emitted, and the model re-asks. So this field is never empty or `(no question text)`. |
| `options` | `Vec<QuestionOption>` (default `[]`) | Each option is `{ id, label, description?, preview? }`. `preview` is markdown the card shows under the option: a picture or a short text sample. It comes from the option's `preview` field in the tool input, which Claude Code's native tool has. Empty for free-text-only prompts. **There is no text-entry option kind**: picking an option resolves to its `label`. So an agent-authored "Other, I'll type it" row hands that literal phrase back as the answer. The card names both real escapes itself: typing in the prompt arrives as a `FreeText` answer, and Cancel arrives as `Canceled`. Every question tool description forbids authoring an "Other" option. |
| `worktree_path` | `Option<String>` | The CC worktree at intercept. `--resume` needs it to find the session JSONL, since CC keys session storage by CWD. `None` for chat-channel questions and requests with no worktree context. |
| `multi_select` | `bool` (default `false`) | `true` when the user may pick multiple options. |

The engine adds one thing to the tool input before it emits the event. Each
workspace picture in `question`, `description` or `preview` gets an *image size
hint* (`system-knowhow/glossary.md`), so `![A](artifacts/approach-a.png)`
arrives as `![A](artifacts/approach-a.png#1600x1200)`. A trigger `condition`
matching those fields sees the hint.

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

**A typed answer can carry images.** `image_hashes` names the blobs the user attached, and `text` may then be empty. A coding agent reads one line per image in its answer, naming the blob's absolute path under `data/blobs/`, and opens it itself. The Lucidos Agent receives the images as image blocks after its question tool's result. An answer naming a blob the workspace never received is refused with a 409.

**Why `Superseded` exists.** The coding agent is parked *inside* the call that asked, and only an answer makes that call return. So a follow-up that cannot be routed as the answer must resolve the question anyway. That releases the agent, which reaches a turn boundary and reads the follow-up. Left open, the thread deadlocked: the follow-up's own `CodingAgentPromptSent` killed the card's buttons and the typed-answer route.

Coding-agent lane only. Any message landing on a question the agent already overtook still supersedes it. So does an engine re-entry when no agent session is live.

Two agent-driven shapes no longer supersede a question the user can still answer. A child-completion wake or an event-wait delivery waits in the agent's input queue (ADR 0255). An agent-sent message, such as a parent's follow-up, becomes a *held message* (ADR 0256). The tool result tells the agent its question was replaced, and that the replacement arrives as its next input.

The card reads "Replaced by your next message", never "Canceled". Like `Canceled`, a supersede emits no resume marker and no `ContinuationRequested`. The follow-up drives the next turn itself. Reasoning and rejected alternatives: ADR 0082.

**Only the engine writes it.** `POST /api/v1/threads/{thread_id}/answer-question` refuses a `Superseded` body with a 400 and leaves the question pending. Only the message router knows a follow-up replaced the question. Use `Canceled` to dismiss a card, or just send the follow-up.

**An engine restart alone does NOT orphan a pending question.** A thread whose newest event is an unanswered `UserQuestionAsked` is a *preserved checkpoint*. Every teardown and recovery path consults one shared predicate (`agent_recovery::thread_has_unanswered_question`) and leaves it alone. That means no boundary `ResponseAborted`, no synthetic `CodingAgentIdled`, no Continue button, and no graceful interrupt to the subprocess. Claude Code's Esc would cancel the pending `AskUserQuestion` and make the agent race past it.

The card stays answerable across the restart, and answering resumes the thread through `ContinuationRequested` then `--resume`. If anything in `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES` lands after the question, the card is dead. The thread then recovers as an ordinary interrupted turn (abort plus Continue).

**Answering a question the subprocess is no longer waiting for.** An answer normally travels *in band*: the engine wakes the blocked PreToolUse hook (Claude Code) or MCP call (Codex), and the answer returns as that tool's result. If the subprocess was torn down while the card was on screen, there is no blocked call to wake. The answer then emits `ContinuationRequested { reason: "answered_after_idle" }`, and the resume message carries the answer itself. `agent_recovery::continue_input_for_reason` builds it over `agent_question::answered_question_recap`: the question text, the answer, and whether the user picked an option or typed a reply.

The resumed agent's hook never re-fires. On teardown, Claude Code closes the pending `AskUserQuestion` in its OWN transcript with a rejecting `tool_result` (`toolDenialKind: "user-rejected"`, `interruptedByShutdown: true`). Its text begins *"The user doesn't want to proceed with this tool use"* and tells the agent to STOP.

That text lives in the CC binary and CC's private JSONL, outside the engine's reach. So the resume message tells the agent the closed call is a teardown artifact: **the user declined nothing, and approved nothing.** Without it, the resumed model read "card closed" plus a bare "continue" as consent, and approved a plan the user had not approved.

**Cancelling one is the mirror case, and it ends the turn.** The cancel stamps the card `Canceled`, and that `UserQuestionAnswered` moves the projection to `running`. With no agent left to interrupt, the Stop handler's settle fallback writes the terminal itself: `ResponseCanceled { user_stop }`. A live agent's interrupt emits the same, so a Cancel reads the same either way. `ResponseAborted { stale_settle }` stays for a `running` row nothing in the request explains. Apply / Discard / Archive keep it too, each carrying its own terminator.

**Pairing guarantee.** A `UserQuestionAnswered` is emitted exactly once per `UserQuestionAsked`, enforced by a unique DB index on `(thread_id, tool_use_id)`. The pair can still be unbalanced:

- The agent process dies before the user answers, the engine restarts, and the coding-agent side never revisits the question. The `Asked` row stays without a matching `Answered`. There is **no engine-side timeout that auto-emits `Answered`** for the AskUserQuestion path. The permission path has an orphan-recovery sweep, but it emits `CodingAgentPermissionResolved { allowed: false, reason: "Coding agent terminated before answering: request expired" }`, a different event family.
- The user closes the card with the cancel control. That fires `UserQuestionAnswered { answer: { kind: "Canceled" } }`, which still counts as a pair.
- A follow-up lands that cannot be the answer. That fires `UserQuestionAnswered { answer: { kind: "Superseded" } }`, also a pair, so watch for both kinds before concluding a question went unanswered.

So treat a long-pending `Asked` with no matching `Answered` as "the user is still being prompted", not as a guaranteed future event.

### `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`

Used by the MCP permission-prompt subprocess (the `mcp__permission-prompt` tool family). Distinct from `UserQuestionAsked`/`UserQuestionAnswered`; the next section says why both exist.

```json
{
  "type": "CodingAgentPermissionRequest",
  "data": {
    "request_id": "req_…",
    "tool_use_id": "toolu_…",
    "tool_name": "Edit",
    "input": { "file_path": "...", "old_string": "...", "new_string": "..." },
    "summary": "Edit /path/to/file.rs"
  }
}
```

```json
{
  "type": "CodingAgentPermissionResolved",
  "data": {
    "request_id": "req_…",
    "allowed": true,
    "reason": null,
    "persist_scope": "session"
  }
}
```

`persist_scope` is `"narrow"` / `"broad"` / `"session"` when the user clicked an "Always allow"-style button. It is `null` for Allow-once, Deny, orphan-recovery, supersession and the session-ended clear. The frontend uses it to check the chosen button on the answered card and strike through the rest.

**The Codex shapes.** Codex raises the same two events through the app-server approval bridge, under its own item names. A `command_execution` carries `{command, cwd, reason?}` and summarizes as `command_execution <command>`.

A `file_change` is the awkward one. `item/fileChange/requestApproval` carries only `itemId` / `threadId` / `turnId` / `startedAtMs` plus a nullable `reason` and `grantRoot` (both `null` in practice), and **no paths at all**. The paths come from the `item/started` notification codex sends for the same item id just before the approval. The app-server driver copies them onto the input, stripped to `{path, kind}` so a multi-file patch's inline diffs never reach the event store:

```json
{
  "type": "CodingAgentPermissionRequest",
  "data": {
    "tool_name": "file_change",
    "input": {
      "item_id": "exec-32d7f4c4-…",
      "changes": [{ "path": "/Users/me/notes.txt", "kind": { "type": "add" } }]
    },
    "summary": "file_change /Users/me/notes.txt"
  }
}
```

`summary` names up to three paths, then appends `+N more`. A trigger condition should key off `tool_name` and `input.changes[].path`, not the prose. If the `changes` list was never announced, the `changes` key is absent. The summary (and the card) then falls back to `reason` / `grant_root` / the bare tool name.

**The agent's own words.** A shell command carries the agent's one line on what it does and why. Claude Code sends it as `input.description` on `Bash`, and Codex as `input.reason` on `command_execution`. The card leads with that line and shows the full command under it. A notification built from this event should prefer it too, since `summary` carries the whole command. Either field can be absent, so fall back to `summary`.

**Supersession.** If the user types a new message while a permission card is pending, the engine resolves the card as `allowed: false` (`reason: "Superseded by a new message"`). It then routes the typed text to the coding agent as a normal follow-up. So the thread is not left stuck on `waiting_for_user_answer` while the agent moves on. This mirrors the AskUserQuestion path, where typing becomes a `FreeText` answer, but a permission has no answer, so it is denied. Emitted from `resolve_pending_permissions_as_superseded` (`engine/cc_permission.rs`).

**A parent's `follow_up_child_thread` does NOT take that path.** Only a human's
message supersedes a pending permission card. An agent-sent message to a
coding-agent child parked on a card becomes a *held message*: the card stays
pending, and the child receives the message once a human answers the card
(ADR 0256). Two related facts about following up a coding-agent child:

- A child parked on a **question** is blocked on a human, not on work. Your
  message is not an answer to that question. It becomes a *held message*: the
  result says `held`, the question stays open, and the child receives your
  message once a human replies. `urgent` does not change that. After a human
  cancels the question, later messages stay held until the human writes again.
  A released message reaches the child as input in the answered turn, or as its
  next turn. Its `CodingAgentInputRead` records when the child read it.
- A follow-up that RACES the child's own finish can produce a completion card
  for the turn you interrupted. That does not mean the redirect failed: the
  redirected turn reports separately when it ends.

For everything else about follow-ups (queued versus urgent, what urgency costs,
why it is a no-op on Codex, and that the child cap never refuses it) see the *child
follow-up* and *urgent follow-up* entries in `system-knowhow/glossary.md`.

**Session-ended clear + non-resurrecting resolution.** A permission card is answered by unblocking an **in-memory** broadcast waiter. It never spawns a resume, unlike `UserQuestionAnswered`, which can resume an already-idled thread. Two consequences:

1. A coding-agent session can **idle** with a card still pending: a parallel subagent's card outlived the main turn, or the turn was canceled. `emit_coding_agent_idled` then clears it via `resolve_pending_permissions_as_session_ended`, whose `reason` starts "Coding agent session ended before answering". So a finished thread leaves no clickable card behind.
2. The projection flips a thread to `running` on `CodingAgentPermissionResolved` **only from `waiting_for_user_answer`**. A resolution on an idle or terminal thread leaves the status unchanged: a stale click hours later, or the session-ended clear itself. That keeps a stale tap from making a dead `running` thread.

The same non-resurrecting rule applies to the chat `CommandPermissionResolved` / `McpPermissionResolved` lanes (shared projection arm).

**Only a Deny click reads as "User denied".** `DENIAL_REASON` ("User denied") is reserved for an explicit Deny. Two other endings return `allowed: false` with a neutral reason instead:

- **An engine restart.** The in-memory broadcast channel closes, so the waiting `prompt_coding_agent_permission` call returns `RESTART_INTERRUPT_REASON` ("Interrupted by an engine restart, not a user decision").
- **A card the engine withdraws**, because a new message superseded it or the session idled. The agent reads that sweep's reason plus "not a user decision".

The chat agent's command and MCP lanes follow the same rule. These reasons are only what the agent reads in the MCP or app-server response. The persisted `CodingAgentPermissionResolved` reasons above (orphan recovery, supersession) are unchanged.

A tool call that was *running* at teardown gets no such reason. Claude Code closes it in its own transcript as "The user doesn't want to proceed with this tool use". It adds `[Request interrupted by user for tool use]`, and the resumed session replays both. So every resumed turn whose gap ends in a restart abort carries a restart line in its turn-gap note (see § "The turn-gap note" below). The line says the shutdown wrote that text, the user refused nothing, and the call is safe to run again. Without it, the resumed agent reads a rejection and abandons its plan.

The recovery system prompts (`recovery_system_prompt` and siblings in `engine/agent_session/prompts.rs`) also carry a **restart-not-rejection note**, but only a recovery spawn uses them.

**Unattended auto-resolution (no card, never hangs).** A permission card needs a human to click it. A coding-agent thread launched by a **trigger** has none, so before rendering a card the engine asks "is anyone here?". It walks the spawn tree from this thread up to its root, via the persisted `MessageOrigin` chain (`cc_permission::resolve_attend_mode`). It hops only where the thread also carries the `parent_thread_id` callback linkage. A *top-thread* names its *spawning thread* for display but is not in its privilege tree, so the walk stops there.

Both top spawn routes stop the walk, the `spawn_thread` tool and `lucidos spawn-thread`. Either way the new thread asks a human.

A human act stops it too. At each thread, the walk first checks for a `MessageReceived` with a human origin or a `UserQuestionAnswered` with a human actor. Either one, at any time, makes the session interactive. So a trigger that asks the user a question, then spawns a coding agent from the answer, spawns an interactive one. So does a trigger-rooted coding-agent thread the user starts writing in.

If the root is a **human device**, the session is *interactive*: emit a card, wait indefinitely. If the root is a **trigger/scheduler**, the thread is *unattended*, whether trigger-fired or an agent-spawned sub-thread of one. The engine then resolves the request from the originating trigger's *side-effect grant* (see `triggers.md` § "Side-effect grant") plus a static benign check:

- **benign in-workspace work** (a read, an in-workspace write/edit, git, `lucidos data write` to `data/`) → auto-**allow**;
- an **irreversible side-effect** (email / external API / cloud CLI / out-of-workspace destruction / other): auto-**allow** if its category is in the trigger's grant, else auto-**deny**. The agent gets the denial and routes around it or reports the step failed. Unlike the chat command guard, this denies the single request and does **not** fail the whole session;
- a **catastrophic** command → auto-**deny**, regardless of grant;
- a shape the command guard's static pass **refused to settle** → auto-**deny**, regardless of grant. A refusal means the command's head is not what runs, or not all of it. The full set:
  - command substitution;
  - a code-injecting `VAR=value` preamble;
  - a path-qualified command head (`./x`, `bin/x`);
  - a redirect target outside the workspace;
  - an out-of-workspace path under a head that can write one (`sort`, `uniq`, `tree`, `xxd`, `yq`, `base64`, `less`, `curl`, `wget`);
  - a create head (`mkdir`, `touch`) pointed outside the workspace;
  - `git -c` / `--config-env` / `--exec-path`, or a git `--output` flag;
  - a `command` field the engine could not read.

  A merely UNRECOGNISED head (`cargo build`, `npm test`, `make`) is NOT one of them: a missing allowlist entry costs a judge call, never safety, so it still auto-allows. See ADR 0002 § Addendum (2026-08-24).

**The refusal set is coarser than an attack shape, and that is the accepted cost.** Three ordinary things are refusals: `cargo build > /tmp/x.log 2>&1` (the redirect this engine's own coding-agent prompt asks for), `./scripts/e2e.sh`, and `sort /etc/passwd`. Separating those from `sort -o /etc/crontab data/f` needs per-head flag arity, which the head lists exist to avoid. An unattended session is denied them and retries, and a deny costs one request rather than the run. To read a file outside the workspace unattended, use `cat` / `head` / `grep`: plain read-only heads that stay on the fast path.

**An auto-ALLOW emits no events** (the same silent fast path session-allow uses). **An auto-DENY emits the ordinary `CodingAgentPermissionRequest` + `CodingAgentPermissionResolved` pair**, back to back, with the command redacted as on the card path. So the timeline says what the engine refused and why. The pair renders as an already-answered card, so it never parks a trigger thread nobody is watching. Under its buttons the card shows the first sentence of the engine's reason, as for every resolution a human did not click. So an engine deny never reads as the user's Deny.

**A trigger subscribed to `CodingAgentPermissionRequest` therefore fires for these too**, on a request the engine already answered. Pair it with the resolution, which lands in the same turn, to tell an engine-answered deny from a card a human still owes.

A **file write is judged over its whole target set**. This matters for Codex, whose `file_change` can name several files in one approval. *Any* target outside the workspace root makes the request out-of-workspace destruction (grant-gated), and only an all-in-workspace set is benign. A Codex `file_change` with unknown paths is grant-gated too, since codex raises that approval only when the patch escaped its sandbox.

This covers both backends: CC's MCP path and the Codex app-server bridge funnel through one engine function, `prompt_coding_agent_permission`. A user-rooted tree stays interactive even when an agent spawned the leaf coding-agent thread, so a human watching can still answer. Classification is static-only: it reuses the *command guard*'s `static_classify` / `fallback_classify`, with no LLM judge. The decision derives from persisted events plus the in-memory trigger registry, so it survives an engine restart. See `cc_permission::{resolve_attend_mode, classify_coding_agent_request}` and ADR 0002 (Phase 5 addendum).

**In-worktree writes never render a card.** Claude Code auto-approves in-cwd writes under `--permission-mode acceptEdits`, **except** under `.claude/` and `.git/`. Those go through the permission-prompt tool in every mode, whatever `--allowedTools` says. Lucidos keeps its agent configuration in `.claude/`, so editing a rule or skill cost a click per save. The persisted "Always allow" scopes cannot suppress it, which is why those cards hide the broad button.

So the engine answers first. A **file write whose target resolves inside the session's own worktree** is auto-allowed with no card and no events. That is safe because the worktree is disposable and every change in it is reviewed in the Diff before Apply. Containment is **resolved against the real filesystem, not matched lexically**. The engine canonicalizes the worktree root and the target's longest existing prefix, so a symlink inside the worktree cannot launder an outside write. Four deliberate limits:

- A `..` component fails containment.
- A relative path fails closed. Resolving one needs the agent's cwd, which is `data/apps/<id>` for an app coding-agent thread. Both backends send absolute paths anyway.
- Any `.git` path component still renders a card, checked before *and* after symlink resolution. Git metadata is not in the reviewed diff, and a written hook would run on the next commit.
- Commands (`Bash` / `command_execution`) are never covered, so a shell command touching `.claude/` still asks.

A write **outside** the worktree, such as the user's global `~/.claude/settings.json`, still asks. See `cc_permission::{worktree_write_auto_allowed, path_inside_worktree}`.

**Two folders join the worktree as *working directories*.** The engine grants them in `cc-settings.json`, so reaching either raises no card: the workspace's `data/` tree, and the OS temp dir. The `--permission-mode` above is a preference, the *coding-agent permission mode*. Under `auto`, Claude Code's own classifier decides instead of carding.

**"Allow for this thread" survives an engine restart.** The grant persists as the resolution's `persist_scope: "session"`. On the first prompt after a restart, `cc_permission::hydrate_session_allows` refills the thread's in-memory set from the event store. It re-derives each pattern through the same `derive_allow_pattern` the grant used, so the two cannot drift. Only `allowed: true` **and** `persist_scope: "session"` rehydrate. None of these do: an Allow-once, a Deny, the `narrow`/`broad` scopes (which live in `cc-allowed-tools`), and the engine's own supersession / session-ended / orphan-recovery resolutions.

**"Always allow" binds the session it was clicked in.** Both persisted scopes append a pattern to this workspace's `cc-allowed-tools`, which Claude Code reads as `--allowedTools` at spawn. That flag is frozen for the subprocess's life, so the engine's gate also reads the file itself, on every prompt. The grant covers the running session, and every other live thread in the workspace.

It honours a stored pattern only where `derive_allow_pattern` would have produced it, which is the codebase's own record of what CC respects. A bare `Edit` / `Write` / `NotebookEdit` / `ExitPlanMode` line covers nothing. A `Bash` command touching `.claude/` or `.git/` still cards, and the Codex tools are untouched. The gate sits BELOW the unattended one, so a workspace grant never answers for a trigger-rooted session. A catastrophic command is still denied, whatever the file says. See `cc_permission::persisted_allow_covers` and ADR 0125.

## Question vs. permission: why both event families exist

Four user-facing prompt mechanisms ride on two event lanes:

1. **CC's native `AskUserQuestion` tool.** CC calls it for structured input mid-turn ("which approach", "is this OK to do"). Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = claude_code`. The `question` payload field is the literal prompt text CC supplied.
2. **Codex's `ask_user_question` MCP tool.** Same wire shape and QuestionCard, one question per call, raised from a Codex session via the `lucidos` MCP server (`lucidos mcp-permission-server`). The MCP server long-polls the endpoint CC's hook uses and returns the answer as the tool result, so the Codex turn continues in place. Fires with `meta.channel = claude_code` (the coding-agent channel). **Codex-only, and enforced**: CC spawns the same binary with `--permission-only`, so a CC session lacks the tool. In CC, every MCP tool call would cost a permission click.
3. **Chat agent's `ask_user_question` tool.** Same wire shape and QuestionCard, raised by the in-process chat agent. Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = chat`. Its optional `message` arrives first, as one `TextStreamed` in the asking turn, just before the first card. The tool blocks on the question wait registry, then returns the joined `{question_text: label}` map as the tool result on the same turn. Use it from chat threads when a button-driven answer beats typing (see `crates/lucidos-engine/src/llm/tools/misc.rs` § `ask_user_question_tools`).
4. **Coding-agent permission prompts.** Authorization for one tool call, possibly persisted ("Always allow Bash(git:*)"). Fires `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`. Two raise paths share the engine machinery. One is CC's MCP permission-prompt subprocess, consulted when CC would run `Edit`/`Write`/`Bash` on a path with no static allow rule. The other is the Codex app-server approval bridge (`item/commandExecution/requestApproval` / `item/fileChange/requestApproval` JSON-RPC requests under `approvalPolicy: on-request`).

Both render as a card the user must act on, but they are two separate event lanes. **A "permission prompt" is not a third event family**: it is whichever of these two raised it.

In practice:

- "Notify me when a coding agent is asking for permission" means one of two subscriptions. `UserQuestionAsked` covers AskUserQuestion and any permission-style prompt routed through the question registry. `CodingAgentPermissionRequest` covers true permission prompts. Pick by what the user wants to act on, or wire a trigger to each.
- `CodingAgentPermissionRequest` reaches a human card **only on interactive (human-rooted) sessions**. An unattended trigger-rooted thread auto-resolves (see "Unattended auto-resolution" above): an auto-allow emits nothing, and an auto-deny emits an already-answered pair. Nobody is there to notify, and the run does not stall.
- Lucidos does NOT seed a default question-push trigger. Workspaces opt in by creating a user trigger (see `triggers.md` § "Worked example: push when agent needs me").

## The error gap

There is no `CodingAgentErrored`, `CodingAgentFailed`, or `CodingAgentCrashed` event. `rg -n "CodingAgent(Error|Errored|Failed|Crashed|Aborted|Canceled)" crates/lucidos-engine/src/` finds zero matches.

When a coding-agent session fails mid-turn (upstream API error, OOM-killed bash, empty assistant text on a non-cancel turn), `classify_result` (`agent_session/lifecycle.rs`) handles it. The engine:

- emits a chat-side `ResponseFailed { error }`, which sets the thread's projected `status = 'failed'` (the red error dot in the thread list);
- emits `CodingAgentIdled { has_changes: <whatever was on the branch> }`, so the dispatcher closes the turn and the UI leaves the "Working" state. This bookkeeping idle does NOT downgrade `failed` back to `idle`.

**A turn's verdict is sticky.** There are two. `failed` means the turn errored, or was interrupted with nobody coming back for it. `paused` means the user's own version switch interrupted it and the engine is resuming it.

Only a real *start* event clears one: `MessageReceived`, `CodingAgentUserMessageSent`, `UserPromptInjected`, `CodingAgentPromptSent`, `ContinuationRequested`. Every event that merely *closes out* the ended turn leaves the verdict alone, via the shared `preserving_verdict` guard (`engine/event_bus/mod.rs`). Those are the trailing activity stream, `ChangeProposed`, `CodingAgentIdled`, `SessionEnded` and `ResponseCanceled`. So the red dot (`failed`) or pause glyph (`paused`) stays until the user sends a follow-up or clicks Continue.

**`paused` is a promise of an auto-resume, and only that.** `AbortCause::status_sql()` splits on `AbortCause::promises_auto_resume()`, which reads the abort's ACTOR as well as its cause. A `ResponseAborted` carrying `EngineShutdown` **and** a device actor is the *Switch to new version* teardown. That is the one interruption the engine brings back by itself, and it settles the thread at `paused`. `StaleSettle` keeps the cancel-style idle mapping.

Everything else settles at the red `failed`, because nobody is coming back for it:

- `SafetyNet`, `ProcessKilled` and `SessionDropped`;
- a system-actor `EngineShutdown`, the shutdown fallback for a thread that started after the restart pre-emit;
- `RecoveryAfterRestart` in both roles: the boot sweep's crash boundary, and the boot floor *withdrawing* a promise it could not keep.

The frontend withholds the **Continue** button with the same predicate (`abortPromisesAutoResume`). So a paused thread never offers Continue, and a thread offering Continue never reads paused.

**A pending change does not change that answer.** The status never becomes `waiting` for one. The thread list ranks `paused` and `failed` above the changes dot by itself, and `coding_agent_proposed` says a change is there. `waiting` is not a value `preserving_verdict` protects, so the dying subprocess's drain would overwrite it with `running`. It would also hide the thread from the boot floor, which selects on `status = 'paused'`.

The guard matters most for a coding agent, because its subprocess outlives the terminal event. A *Lucidos Agent* turn's loop emits nothing after its own terminator. A coding-agent turn emits four more things.

An interruption (engine restart, watchdog kill) emits `ResponseAborted` while the agent is still alive. `external_terminal_emitted` then suppresses the duplicate *terminal* and drops the agent's further text, thoughts, tool calls and tool results. That includes any reply to the restart's interrupt, which Claude Code reports to the model as a user rejection. Only text streamed before the interruption can still land, as a `CodingAgentTextStreamed` milliseconds later.

Then come the `ChangeProposed` the agent commits on its way out, `CodingAgentIdled` and `SessionEnded`. The guard keeps all four from walking the thread back from `failed` to `idle`. The drain lives in the shared `agent_session` layer, so Claude Code and Codex behave identically.

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

For a workspace-defined failure name, `lucidos events emit CodingAgentFailureObserved {...}` (or the `emit_event` LLM tool) writes a `SystemEvent::DomainEvent` that always reaches the matcher. Use it when the engine's ThreadEvents lack the discriminator you need (e.g. OOM versus API-503).

## Notes on `CodingAgentPromptSent`

This is NOT the user typing into a coding agent: that fires `CodingAgentUserMessageSent`. `CodingAgentPromptSent` marks prompts the engine synthesized:

- the merge-conflict explanation injected after `MergeConflictDetected`;
- the `MissingHardeningDetected` recovery prompt asking the coding agent to run `/harden`;
- the empty `CodingAgentPromptSent` right after `UserQuestionAnswered`, a "thinking" placeholder while the coding agent processes the answer. It is skipped for two answer kinds, since no turn of their own follows. On `Canceled`, the cancel-stamp path (`claude_code_stop`, `archive_thread`) tears the agent down, so the marker would strand as an empty step. On `Superseded`, the replacing follow-up emits its own `CodingAgentPromptSent` moments later. See `emit_resume_marker_for_cc_answer` in `crates/lucidos-engine/src/engine/agent_question.rs`.

It carries `origin: Some(MessageOrigin::Engine { reason: ... })` on the wire. Workspaces rarely need to subscribe: it is an audit trail event, not a lifecycle signal.

## Resume-time notes prepended to the next prompt

`--resume` replays the prior conversation, but NOT what changed on disk or in `main` since. So on resume the engine prepends up to three short `[Note from engine: …]` blocks to the user's next message. They are NOT persisted events. They ride only on the in-memory prompt text, built in `build_resume_prompt_text` (the single injection point, for both Claude Code and Codex). They appear only when the user's message is non-empty:

- **Branch adoption**: the worktree was switched to a new branch holding the agent's prior work; the engine adopts it and says so.
- **Turn gap**: what the user or the engine did to the agent's work while it was idle. See below.
- **External edits**: the worktree changed between turns, detected by diffing against the `worktree_head_sha` on the last `CodingAgentIdled`. The wording names no cause. The detector sees only a SHA and a `git status`, so it cannot tell a hand edit from an engine reset. When the turn-gap note already states why HEAD moved, one line is suppressed: the `HEAD moved (no log available)` fallback, the signature of a backwards reset. A real commit log and any uncommitted change are still reported.

**A non-empty block closes by disclaiming the topic.** The notes quote commit subjects, branch names and file paths. The next message often leans on a pronoun, and those nouns can capture it. So the block's last line calls those lines engine status, not the subject. A turn with no note gains nothing.

### The turn-gap note

The **turn gap** is the window between the agent's previous turn boundary and the current one. The resumed agent cannot see events in it, because `--resume` replays only its own conversation. Left unsaid, the agent might offer to Apply a discarded change or vanished commits, or treat reverted work as still in `main`.

| Event in the gap | What the note tells the agent |
|---|---|
| `ChangeApplied` | Merged into `main` and the worktree reset to match. Lists the merged commit subjects with the short post-merge `main` SHA. Not pending anymore. |
| `ChangeDiscarded` | Discard reset the change's branch to `main` and cleaned the worktree, so those commits are gone. Names the branch, and says whether it is the session's own branch or a stale change on a different one (the reconcile path discards siblings on other branches, and that must not read as "your work is gone"). Not pending: do not offer to Apply it. |
| `ChangeReverted` | The change had applied and has since been undone in `main` by revert commits, so `main` no longer contains that work. The branch and worktree were not touched, because a revert runs in the main repo. |
| `ChangeApplyFailed` | An Apply attempt did not land and the change is still pending. The engine's error is quoted and attributed as the message shown to the user, never rendered as an instruction to the agent. |
| `WorktreeCleaned` | The cleanup worker reclaimed the worktree. Tier 1 stripped `target/` / `node_modules/` / `.lucidos/cache/`, so the next build starts cold; tier 2 removed and recreated the whole worktree, so untracked files are gone, and may have deleted a fully-merged branch. |
| `ChildThreadStopped` | A user Stop paused one of the agent's child threads. The child is alive and waits for the user; a completion card follows when it next finishes, or a canceled one if the user archives or discards it. It says not to roll back the child's work, respawn it, or follow it up meanwhile. |
| `BackgroundBashCompleted` | A background task the agent spawned ended and no wake reached it. With `abandoned: true` the engine stopped mid-run, so there is no verdict: it says to drain the task rather than report it as passed or failed. Otherwise the task finished normally and the line carries its status. |
| `ResponseAborted` with cause `engine_shutdown` or `recovery_after_restart` | An engine restart stopped the agent's last turn mid-turn. The agent's transcript may show a tool call as rejected ("The user doesn't want to proceed … STOP … wait for the user") or a `[Request interrupted by user …]` line. The line says the shutdown wrote that and the user refused nothing. It says a running tool call did not complete and is safe to run again, after checking for partial side effects. |

**The restart row reads only the newest coding-agent terminal in the gap.** Claude Code writes the rejection text itself at teardown, and the engine cannot change it. So the resume names the real cause, or the agent reads a restart as the user saying no. The row stays out once a later coding-agent terminal follows the abort, since a resumed turn already ran past the restart. Other abort causes, user cancels and chat-channel aborts never produce it. It rides every resume path: the automatic resume after a switch, a Continue click, and the user's next message after a crash.

**The `BackgroundBashCompleted` row skips what an event wait delivered.** A coding agent's `lucidos background-task run` arms an *event wait* on the completion, and the re-opened turn carries the delivery as its prompt. So the note drops a completion an `EventWaitDelivered` names. Only an undelivered one reaches the note, such as a task whose wait a subscription cap refused. A task the agent stopped itself is left out too: the stop stood its watch down, so the killed completion is not news (ADR 0369). See § `BackgroundBashCompleted` in `system-knowhow/thread-events.md`.

Deliberately NOT in the note, because another mechanism already delivers them:

- `ChildThreadCompleted`. It is itself a coding-agent turn origin, and wakes the parent carrying the child's summary.
- `CodingAgentPermissionResolved`. Delivered in-band to the waiting call.
- `UserQuestionAnswered`. Delivered in-band while the subprocess is alive. When it is not, it arrives as the `answered_after_idle` resume message's own BODY, see "Answering a question the subprocess is no longer waiting for" above. That body carries the same `[Note from engine: …]` marker but is not one of the three reconciliation notes here, and `agent_recovery::continue_input_for_reason` assembles it rather than `build_resume_prompt_text`.

Deliberately NOT in the note, because one would add nothing:

- `MergeConflictDetected` and the merge-resolution events. The conflict session spawns with a purpose-built system prompt naming the conflicted files.
- `ChangeHardened`. The agent's own `/harden` caused it.
- `ChangeSummarized`. It names the agent's own commits back to it.
- `ChangeSetAside` / `ChangeBroughtBack`. The change and its branch are untouched, and the agent's next proposal brings a set-aside change back by itself.
- `CodingAgentSettingsChanged`. The resumed process already runs with the new model.
- The cosmetic thread events.
- `ThreadArchived` / `ThreadDiscarded`. Terminal, and archive's pending-change discard already arrives as `ChangeDiscarded`.

The note is **stateless and self-clearing**. It surfaces every covered event whose `sequence` falls between the *previous* turn boundary and the *current* turn's triggering event. The engine persists the current turn's origin before resuming the agent, and its `sequence` bounds the window. Anything after the origin (a second message racing the same spawn, a click while the agent starts) belongs to the next turn's gap.

The boundary set is every event type that can originate a coding-agent turn (`MessageReceived`, `CodingAgentUserMessageSent`, `TriggerStarted`, `ChildThreadCompleted`), plus `CodingAgentPromptSent`. Each turn's origin becomes the next turn's boundary, so an event surfaces exactly once. No new event, projection column or migration backs it: the `events` table is the cursor.

## Recipe-shaped guidance

**Every recipe below is a *trigger*, the right shape only when the reaction should outlive the conversation and reach the user as a notification.** To tell the user **in the thread they are typing in** ("let me know here when a coding agent edits code"), use the `await_event` tool. A trigger runs in its own thread and cannot report into an existing conversation. The `on` entries below transplant verbatim into an `await_event` call. Pick the mechanism by where the answer must land, then by how long it must last. See `system-knowhow/triggers.md` § "When a trigger is the right answer".

For the trigger config field reference (cron format, the `on` subscription list, per-entry `condition` operators), see `system-knowhow/triggers.md`. A condition key is a field path: a dot reads one level down. Operators: `$eq` `$ne` `$lt` `$lte` `$gt` `$gte` `$in` `$nin` `$regex` (a bare value is `$eq`), plus `$or` in key position taking a list of conditions. See `system-knowhow/triggers.md` § "What a condition can say" for the full language.

### Notify when a coding agent is waiting on the user

```yaml
on:
  - event_type: UserQuestionAsked
run:
  intent: "Notify me when the coding agent has a question waiting for me. The push should deep-link straight to the question: tapping it takes me to the originating thread and pulses the question card on land."
```

The `tap` + `event_id` pair makes the push deep-link: the tap opens the originating thread *and* pulses the matching event card. Without them the push opens the inbox modal. See `triggers.md` for the full pattern.

To scope to a specific coding-agent session, add a per-entry `condition`:

```yaml
on:
  - event_type: UserQuestionAsked
    condition:
      cc_session_id: "abc123-…"
```

Conditions are pure payload filters: the event's own payload fields, plus `thread_id`, which the engine supplies for every thread event. `cc_session_id` is on `UserQuestionAsked`'s payload, so this works. Nothing else about the thread is reachable (no title, no app id, no status). A **domain event** has no `thread_id`, since it belongs to no thread.

### Notify when a coding-agent session finishes / produced changes

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      has_changes: true
run:
  intent: "Tell me the coding agent finished and left a change to review."
```

Add `is_external_repo: { $ne: true }` to scope to the engine repo, or `requires_restart: true` for changes that need a new engine version. The same condition language works on every payload field.

### Wait for one named coding-agent session to finish

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      thread_id: "<uuid>"
```

The same entry works as an `await_event` subscription. That is how a chat thread waits for a coding-agent session it did not spawn: list the running ones (`threads` list, `status: ["running"]`), then subscribe per session. Two caveats:

- `CodingAgentIdled` is a **turn boundary**, not a session terminator, so a session that gets a follow-up emits another one later.
- The first match spends a subscription. To wait on several sessions, re-list on each wake and subscribe again while any are still running.

### Notify when a coding agent errors

```yaml
on:
  - event_type: ResponseFailed
run:
  intent: "Send me a push notification with the failure error."
```

`ResponseFailed` fires for both chat and coding-agent failures. See "The error gap" above for what "error" means here, and for finer-grained alternatives.
