---
name: Coding Agent Lifecycle Events
description: ThreadEvents a Claude Code or Codex session emits, which ones the scheduler blocks, and how to spawn one with `run_coding_agent`. Load for "notify me when a coding agent finishes / asks a question / errors", "which folder should the coding agent edit", "wait until the agent is idle", "watch for a permission prompt", "CC session", "AskUserQuestion".
---

# Coding Agent Lifecycle Events

`CodingAgent*` is the engine's umbrella name for events emitted by a coding-agent session — Claude Code or Codex. Both backends translate their CLI output into the same `ThreadEvent` enum (the variants carry a `coding_agent: CodingAgent` field that distinguishes them; default is `ClaudeCode` for legacy DB rows; the wire field has `#[serde(alias = "agent")]` so rows persisted before the rename still decode).

The session's *worktree* wraps the enclosing git, which differs by `coding_agent_kind`:

- `lucidos` (default) — full checkout of the Lucidos source repo. Only available on an install whose engine was launched from a Lucidos source checkout; a packaged install ships the binary alone, so there is no platform source, the compose destination picker hides the "Lucidos source" target, and `run_coding_agent` with `folder` omitted is refused.
- `app` — sparse-checkout of the workspace git narrowed to `data/apps/<id>/` (see *app worktree*). Apply ff-merges to the workspace git's local `main`; no engine restart; `/harden` does not run; `AppUiRefreshRequested` emits when iframe-bundled files change.
- `external` — full checkout of the user-registered external git repository. No Apply / Discard surface.

`coding_agent_kind` ships on every `SessionStarted` event and is persisted in `thread_summaries.coding_agent_kind` so the apply path can dispatch correctly without re-reading the event log.

### Choosing `folder` when you spawn one

`run_coding_agent`'s `folder` argument is what selects the kind above, so pick it
before you call rather than discovering the refusal afterwards. Ambiguous? Ask
which folder first.

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

**Cross-workspace.** Set `workspace` to the target workspace's basename and the
tool POSTs to that engine, where the session lands. It requires
`relation: "top"`, because a child auto-resume callback does not cross
workspaces, and child plus cross-workspace is refused with an error. `folder`
then resolves on the TARGET workspace, so the app must be installed or the repo
registered *there*, and that engine applies its own source-checkout check. This
is the route that stays open on a packaged install: the refusal above is about
THIS install, not about the caller.

What happens after Apply (which changes need a rebuild, who triggers the restart,
how to verify a new build is live) is the chat system prompt's ENGINE RESTARTS
and APPLYING & VERIFYING CHANGES sections, not something to restate here.

**Backend selection (Claude Code vs Codex).** Which backend drives a thread is chosen at the thread's FIRST send (the compose destination picker's coding-agent chip → `coding_agent` on the chat request; default `claude-code`, remembered per workspace via the `coding_agent_default` preference), shipped on `SessionStarted.coding_agent`, and persisted in `thread_summaries.coding_agent`. The value is **locked**: follow-ups and recovery always resume on the stored backend (the other backend has no session to resume, so a flip would silently lose the conversation), and a follow-up requesting a different backend is rejected with `409 Conflict`. Backend differences a workspace can observe:

- Codex emits coarse-grained tool events — `command_execution`, `file_change`, `mcp_tool_call`, `web_search`, `todo_list` — instead of CC's named tools (`Bash`, `Read`, `Edit`, …). Trigger conditions on `CodingAgentToolCalled.name` must match per backend.
- `UserQuestionAsked` / `UserQuestionAnswered` fire for **both** backends. CC routes through its `AskUserQuestion` PreToolUse hook; Codex calls the `ask_user_question` tool on the `lucidos` MCP server (one question per call), which hits the same blocking engine endpoint — the answer returns inside the same Codex turn as the MCP tool result, no resume round-trip. Neither backend emits a `CodingAgentToolCalled` for the question tool itself (the question card IS the surface; tool names `AskUserQuestion` and `mcp__lucidos__ask_user_question` are suppressed) — wire triggers to `UserQuestionAsked`, not to the tool name.
- `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved` fire for CC always, and for Codex under the default `app-server` protocol (`approvalPolicy: on-request` — sandbox-escaping commands and out-of-worktree file changes raise the same PermissionCard CC's prompts use, with Codex item names `command_execution` / `file_change` as the `tool_name`). Under the `LUCIDOS_CODEX_PROTOCOL=exec` escape hatch Codex runs non-interactively with the OS sandbox (`--sandbox workspace-write`) as the only guard and emits **no** permission events.
- `/harden`, Apply / Discard, worktrees, branches, and every event in this file are backend-agnostic — same lifecycle, same payload shapes, with `coding_agent` distinguishing the producer.

This file enumerates the full set, marks which are safe to subscribe a trigger to, and points out two recurring sources of confusion: there is no `CodingAgentPermission*Event` separate from `UserQuestionAsked`, and there is no `CodingAgentErrored` event at all.

For the **full ThreadEvent enum** (chat-side, lifecycle, changes, background bash, plugin / repo, transient SSE-only commands — everything outside the coding-agent + `UserQuestion*` slice this file covers), see `system-knowhow/thread-events.md`. That file also documents the persistence + triggerability status for every variant in one place; this file is the coding-agent deep-dive.

For trigger config syntax (cron vs the `on` subscription list, per-entry `condition` filters, `run.intent` discipline), see `system-knowhow/triggers.md`. For event-store column shape and the chat-side terminator events (`ResponseGenerated` / `ResponseFailed` / …), see `.claude/rules/db.md`.

## Triggerability: blocklist semantics

The scheduler forwards persisted ThreadEvents to the trigger matcher, unless they sit on a small **per-token streaming blocklist**. That is `ThreadEvent::is_per_token_streaming` in `crates/lucidos-engine/src/engine/thread_events/event_impl.rs`, gated at the `BusEvent::Thread` arm of the scheduler subscriber in `crates/lucidos-engine/src/scheduler/mod.rs`. Two coding-agent entries sit on it, `CodingAgentTextStreamed` and `CodingAgentThoughtStreamed`. Those are the two coding-agent variants a workspace cannot subscribe to today.

That means right now (each line is one entry inside a trigger's `on` list, see `system-knowhow/triggers.md` for the full subscription shape):

- `event_type: UserQuestionAsked` — works. Wire this to a trigger that calls `send_notification` with `tap: { kind: 'navigate', to: { target: 'thread', id: '<thread_id>', event_id: '<source_event_id>' } }` (event_id taken from the trigger's `Source event id:` line) to push the user with a deep-link straight to the question.
- `event_type: CodingAgentIdled` — **works**. Pair with the entry's `condition: { has_changes: true }` to scope to "the coding agent finished and left work to review."
- `event_type: CodingAgentPermissionRequest` — **works**. Lets a workspace react to "the coding agent is asking permission for a tool call."
- `event_type: CodingAgentToolCalled` / `CodingAgentToolResult` / `CodingAgentPromptSent`: **works**, but these are per-action and chatty. Always pair with the entry's `condition:` (e.g. `name: "Bash"`) or the trigger fires many times per turn. A condition key is a field path, so the command text inside `args` is filterable too: `{ "args.command": { "$regex": "cargo test" } }`.
- `event_type: CodingAgentTextStreamed` / `CodingAgentThoughtStreamed`: does not fire. Per-token streaming is the only `ThreadEvent` the scheduler blocks, so subscribing to either is a no-op.
- `event_type: <any chat-side lifecycle event>` (`ResponseGenerated`, `ResponseFailed`, `ChangeApplied`, …) — works; same blocklist semantics. See `system-knowhow/thread-events.md` for the full set.

The blocklist is not the only gate. An event a trigger's own run emits dispatches one level deeper in the chain, and `MAX_EVENT_TRIGGER_DEPTH` (3) stops it there. That matters most for `CodingAgentIdled` and `ResponseGenerated`, which a trigger's own run emits on its own thread. See `system-knowhow/thread-events.md` § "Today the scheduler uses a blocklist" for the cap, and `system-knowhow/triggers.md` for the authoring rule the cap only backstops.

## The full enumerated list

All variants below are defined on `ThreadEvent` in `crates/lucidos-engine/src/engine/thread_events/event.rs`. Each has a `#[serde(alias = "ClaudeCode<X>")]` for the legacy pre-rename name: write new code (and new triggers) against the `CodingAgent*` form.

### Persisted, low-volume — terminal-state or one-per-turn

| Event | When it fires | Volume |
|---|---|---|
| `CodingAgentUserMessageSent` | A user message was relayed into the agent's input stream. One per user-typed message on a coding-agent thread. | One per user message |
| `CodingAgentPromptSent` | An engine-synthesized prompt was injected (orphan-recovery, hardening retrigger, merge-conflict explainer, post-question continuation). Carries an `origin: Option<MessageOrigin>` so the route popover can render "Engine · …". Persisted for audit; not rendered as a chat bubble. | One per engine-driven injection |
| `CodingAgentSettingsChanged` | Two roles. (1) User changed model, reasoning effort, or permission mode mid-session via the in-thread control. (2) Emitted once at backend init carrying `cc_session_id: Some(..)` (and `claude_config_dir`, the `CLAUDE_CONFIG_DIR` the session was created under) when available — the moment the backend reports a resumable session id — so both are durable in the event store *before* the first `CodingAgentIdled`. Persisted so settings survive idle exit + respawn, so a mid-turn engine restart can still resume (`lookup_latest_cc_session_id` reads `cc_session_id` from this event as well as from `CodingAgentIdled`), and so a resume re-injects the exact `CLAUDE_CONFIG_DIR` the session's transcript lives under — `$CLAUDE_CONFIG_DIR/projects/<cwd>/<sid>.jsonl` — so a user toggling that env var mid-flight can't strand the session. A coding-agent thread is **pinned to the account (config dir) of its first session**: the engine records the earliest `claude_config_dir` and re-injects it on *every* later spawn (`lookup_pinned_cc_config_dir`), and scopes the auto-detected resume session id to that account (`lookup_latest_cc_session_id_for_config_dir`), so a thread never switches provider after turn 1 even if the global toggle changes between turns. Only a thread's very first turn adopts the live toggle. | Once at init + on each user toggle |
| `CodingAgentPermissionRequest` | The coding agent asked to confirm a tool call. Claude Code raises this through its MCP permission-prompt subprocess (Edit/Write/Bash on a path outside the session's *working directories*, anything under `.claude/` or `.git/`); Codex raises it through the app-server approval bridge for sandbox-escaping commands and out-of-worktree file changes. The user resolves it via `POST /api/v1/permission/<request_id>/{allow,deny}`. **Clickable only on interactive (human-rooted) sessions.** An unattended trigger-rooted session auto-resolves instead. Its auto-allow emits nothing. Its auto-DENY emits this event followed at once by the resolution, so the refusal is on the record (see "Unattended auto-resolution" below). | Per tool call needing consent, plus one per unattended auto-deny |
| `CodingAgentPermissionResolved` | The above request was answered (or auto-resolved by recovery / supersession). Carries `allowed: bool`, an optional `persist_scope` (`narrow`/`broad`/`session`) recording which "Always allow"-style scope the user picked, and a `reason` for failure / orphan-recovery / superseded cases. | Pairs 1:1 with `CodingAgentPermissionRequest`. Three engine paths emit `allowed: false`: orphan-recovery (the agent died first), supersession (the user replied instead of clicking), and an unattended auto-deny. Each carries its own `reason`. |
| `CodingAgentIdled` | **The turn-boundary marker.** Emitted at the end of every coding-agent turn whose Result wasn't an engine-shutdown abort. See "`CodingAgentIdled` semantics" below for the full payload. | One per turn (a session normally has 1–3 across its life; many more if the user keeps replying) |
| `MissingHardeningDetected` | Engine detected that a coding-agent session ended without running the required `/harden` and auto-spawned a recovery hardening session. **Not a session terminator** — the thread stays active until hardening finishes. | Rare; only on the recovery path |

### Persisted, high-volume — pair with `condition:` (or, for streaming, blocked entirely)

These fire many times per turn. `CodingAgentTextStreamed` and `CodingAgentThoughtStreamed` are per-token streaming and are **blocked** by the scheduler, so subscribing is a no-op. `CodingAgentToolCalled` / `CodingAgentToolResult` are per-tool-call (a few to a few dozen per turn): they flow through the matcher, but a trigger without a `condition:` filter will fire on every tool call. Always scope by a top-level field, e.g. `name: "Bash"`.

| Event | When it fires | Triggerable |
|---|---|---|
| `CodingAgentTextStreamed` | Each `text` chunk the coding agent streams to the user. One per assistant-message line / paragraph as the backend writes it. Carries what the MODEL wrote, never the backend's own API-error banner: Claude Code reports an upstream drop as a `<synthetic>` assistant message flagged `is_api_error_message`, and the engine skips it, because the identical string returns as the turn's failure reason and is already rendered from `ResponseFailed`. | **no (blocked: per-token streaming)** |
| `CodingAgentThoughtStreamed` | Each chunk of streamed reasoning/thinking the coding agent produces before its visible output. CC sends it as a `stream_event` `thinking_delta` (text on `delta.thinking`; the persisted CC JSONL keeps only an encrypted signature, so the live stream is the only source); Codex sends `item/reasoning/summaryTextDelta` / `textDelta` (app-server) or a `reasoning` item (exec). Coalesced into a few rows per turn and rendered as the live "Thinking" step's content so a long reasoning pass shows progress. **Live on Codex, dormant on CC.** Codex: both drivers set `model_reasoning_summary=detailed` (codex's default summary mode emits no reasoning notifications at all — verified live on codex-cli 0.142.5), so Codex threads stream reasoning *summaries* into this event. CC: **dormant for the current models — NOT provider-specific:** Anthropic's `thinking.display` defaults to `omitted` on every current model — Fable 5 / Opus 5 / Opus 4.8/4.7 / Sonnet 5 — so the `thinking_delta` carries empty text (signature only) and this event does not fire — on **both** Vertex and the first-party Anthropic API (empirically confirmed), and even with `--thinking-display summarized` forced (an upstream Claude Code limitation in its headless `stream-json` path; the raw chain of thought is never returned regardless — a summary is the most any display mode yields). Switching CC's provider does **not** fix it. See `docs/temporary-measures.md` § `cc-reasoning-dormant`. | **no (blocked — per-token streaming)** |
| `CodingAgentToolCalled` | Each tool invocation the coding agent makes. Carries `name`, `args` (full JSON), optional `description`, and `tool_use_id` so the matching `ToolResult` can be paired even when a permission prompt splits them across exchanges. | yes (use condition) |
| `CodingAgentToolResult` | The result returned to the coding agent for a prior `ToolCalled`. Carries the same `tool_use_id`. | yes (use condition) |

### Transient — never persisted, broadcast over SSE only

| Event | When it fires |
|---|---|
| `CodingAgentThreadSpawned` | A child coding-agent thread (spawned via `run_coding_agent` / `run_thread`) has started — carries the new `cc_thread_id` + `title`. SSE-only: the persisted record of the child is its own thread row. |

### `UserQuestion*` — the question / permission channel

These are NOT prefixed `CodingAgent*` because the same machinery serves any agent that needs to ask the user a structured question. Today three raise paths exist: CC's built-in `AskUserQuestion` tool, Codex's `ask_user_question` MCP tool (on the `lucidos` MCP server — one question per call), and the chat agent's `ask_user_question` LLM tool. All three emit the same `UserQuestionAsked` event and all three route their answer through `POST /api/v1/threads/{thread_id}/answer-question`; the engine branches on `meta.channel` to fire the right resume side-effects (coding-agent channel: resume marker + `ContinuationRequested` respawn if the subprocess is gone; chat: wake the in-process tool waiting on the question wait registry).

| Event | When it fires |
|---|---|
| `UserQuestionAsked` | An interactive question has been raised — by CC's `AskUserQuestion` tool, Codex's `ask_user_question` MCP tool, or the chat agent's `ask_user_question` tool. `meta.channel` records which lane raised it (`claude_code` — the coding-agent channel, both backends — or `chat`). The raising agent blocks while the card is on screen: CC's PreToolUse hook and Codex's MCP server both long-poll the engine's internal endpoint; the chat tool blocks in-process. Resume happens via `POST /api/v1/threads/{thread_id}/answer-question` (which emits `UserQuestionAnswered` and dispatches the channel-specific resume path). |
| `UserQuestionAnswered` | The user (or, on the orphan-recovery path, the engine) supplied an answer. Pairs 1:1 with the matching `UserQuestionAsked` via `tool_use_id`. Carries the originating channel via `meta.channel` so downstream consumers can tell chat answers from coding-agent answers without re-looking-up the `Asked`. |

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

All fields except `coding_agent` are `#[serde(skip_serializing_if = ...)]`-gated and will be missing from the wire when at their zero value (`false` for bools, `None` for `Option`s). Read defensively: `payload.has_changes ?? false`, `payload.worktree_path ?? null`.

**A `condition` on this event can also name `thread_id`, which is NOT in the payload above.** The engine supplies it from the thread the event belongs to, for every thread event, so `{ "thread_id": "<uuid>" }` scopes a wait or a trigger to one coding-agent session. It is a matching-time field only: it is not written to the event row (the `events.thread_id` column is where the thread is stored), so a consumer reading a persisted payload will not find it there.

| Field | Type | When present |
|---|---|---|
| `has_changes` | `bool` | `true` iff the coding-agent branch has a non-empty net diff against its **diff base** — the SAME base the Diff button renders against (`default_diff_base`: `origin/<default>` when the local default branch has diverged, otherwise the local default) — **after** filtering out runtime-only paths (`.lucidos/**` etc. — see `branch_changed_files` + `files_require_restart`). Sharing the base is deliberate: the gate that shows the Diff button and the algorithm that computes the diff are one (`branch_changed_files`), so the button can never light up on an empty diff. Carries forward from a prior idle if the live turn produced no new commits but the branch still has prior work. Drives the `coding_agent_has_diff` projection column (the WaitingBanner Diff button). During a live turn, the worktree post-commit hook can update the same column earlier through `CodingAgentDiffChanged`; `coding_agent_proposed` is still set exclusively by the aggregate `ChangeProposed` (non-empty `change_id`), which only follows when the coding-agent turn ended `Generated` and `may_touch_change_state_at_idle` permits. Aborts / cancels / mid-turn deaths never create an Apply proposal — see the "Changes" section of `thread-events.md`. |
| `is_external_repo` | `bool` | `true` for sessions running against a repo imported via `RepositoryImported` (i.e. not the Lucidos engine repo itself). Authoritative for the `coding_agent_is_external_repo` column the WaitingBanner reads to swap Apply for Done/Archive. |
| `requires_restart` | `bool` | Derived from the same filtered file list — `true` iff at least one changed file matches `files_require_restart` (Rust source, `Cargo.lock`, certain bundled assets). Informational only — `ChangeProposed.requires_restart` is the authoritative source for the `coding_agent_requires_restart` column. |
| `cc_session_id` | `Option<String>` | The CC CLI session id at the moment of idle. `None` for recovery-emitted idles where no live subprocess existed (see "no-branch" and "stuck-session" recovery paths). Not the only carrier: the id is also pinned at `Init` via `CodingAgentSettingsChanged`, so a turn interrupted by an engine restart before its first idle is still resumable — `lookup_latest_cc_session_id` reads the most recent non-null id across both event types. |
| `coding_agent` | `CodingAgent` | `"claude-code"` or `"codex"` (kebab-case wire values). Defaults to `"claude-code"` on legacy DB rows. Has `#[serde(alias = "agent")]` so rows persisted under the older `agent` field name still decode. |
| `reason` | `Option<String>` | **Usually absent.** Stamped only by recovery: `"engine_restart_interrupt"` when a mid-turn-crashed session is surfaced to the UI as "interrupted, click to continue" instead of being auto-spawned. The frontend reads this to render the continue affordance. |
| `worktree_path` | `Option<String>` | Absolute filesystem path of the worktree the agent ran in. Populated by `run_session.rs` for normal turns. **`None`** when the worktree was `worktree remove --force`'d before the idle fired (the "stale session" cleanup path, the no-branch recovery path) and on legacy rows. |
| `worktree_head_sha` | `Option<String>` | Snapshot of `git rev-parse HEAD` in the worktree at idle time, used by the next spawn to detect external user edits made between turns. `None` on legacy rows, when no worktree, or when `git rev-parse` fails (e.g. zero-commit branch). |
| `bg_bash_pending` | `bool` | **Recorded history only — no longer gates proposal or drives UI.** `true` iff the turn ended idle while the chat-agent's `run_bash_background` tool still had a task running. As of the bg-bash-gate removal, `may_touch_change_state_at_idle` ignores background bash entirely: the change proposes the instant the coding agent idles, and correctness is covered by harden-at-apply (an un-hardened change re-runs `/harden`, tests included, before it can merge). The earlier scheme — a projection column + "coding agent waiting on background tasks…" banner + a 5-minute `BgBashWakeRequested` nudge — was removed because that wait was worse than the rare wasted re-harden it prevented (it also wedged permanently when the coding agent verified completion via a shell `ps` check instead of `TaskOutput`). The field is kept on the event so the timeline still records that an idle overlapped a background task. **Startup recovery:** any idle coding-agent thread with a committed diff but no proposed change (e.g. one wedged by the old gate) is re-proposed at boot by `propose_held_back_changes_on_startup` (skip if already-proposed / no diff / external-repo / branch missing / no authored commits left — the branch's work is already merged into main and only an engine back-merge of main remains, whose three-dot diff would otherwise re-surface the already-applied files as a phantom change), emitting `ChangeProposed` with `origin: Engine{ reason: StaleSession }`. |

**Fires on:** every coding-agent turn that ended on a `Result` other than engine-shutdown abort. That covers natural completion, `Failed` (the coding agent errored, OOM, empty assistant text), and user `Cancel` (the `Cancel` button — cancel is treated as a turn boundary, not a terminator). Engine shutdown does NOT emit `CodingAgentIdled` — recovery resumes the session on next start.

**Cancel = Esc (resumable):** the `Cancel` button is a real *interrupt*, not a kill. `POST /api/v1/claude-code/stop` (default, `StopReason::UserStop`) routes through `interrupt_agent`, which forwards CC's native interrupt (the equivalent of pressing `Esc` in the CLI). CC winds down the current turn, emits a `Result`, and the engine emits `ResponseCanceled(UserStop)` **+** `CodingAgentIdled` carrying the `cc_session_id`. The branch is **kept** even with zero commits (`SessionEndAction::KeepCanceledBranch` in `finalize`), so the next message `--resume`s the *same* conversation on the *same* branch — no fresh session, no re-asking. (Apply / Discard / Archive still hard-stop via `stop_agent`; each carries its own terminator.) A bounded fallback escalates to the hard stop only if CC fails to honor the interrupt within ~8s (hung socket, control request ignored while a long tool runs — the watchdog skips while a tool is in flight); even then the turn is stamped `Canceled(UserStop)` so the branch is kept and the session stays best-effort resumable. Before this, the `Cancel` button hard-killed CC and `git branch -D`'d the branch, so the follow-up spawned a brand-new, amnesiac session.

### `UserQuestionAsked`

```json
{
  "type": "UserQuestionAsked",
  "data": {
    "tool_use_id": "toolu_…",
    "cc_session_id": "abc123-…",
    "question": "Which approach should I take?",
    "options": [
      { "id": "opt-0", "label": "Approach A", "description": "…" },
      { "id": "opt-1", "label": "Approach B" }
    ],
    "worktree_path": "/Users/.../worktrees/thread-…",
    "multi_select": false
  }
}
```

| Field | Type | Notes |
|---|---|---|
| `tool_use_id` | `String` | The agent's identifier for this question; used as the unique key in DB and in `UserQuestionAnswered.tool_use_id`. Per-question sub-ids are synthesised as `{outer}#q{i}` so a multi-question batch from a single tool call gets one DB row per individual card. |
| `cc_session_id` | `String` | The CC session id at the moment of intercept — resume pins to it via `--resume`. **Empty string** for questions raised by the chat agent's `ask_user_question` tool (chat is in-process, no subprocess to resume). |
| `question` | `String` | The prompt text shown to the user. **For permission requests, this is the human-readable rendering of the tool-use that the coding agent asked permission for** — see "Question vs. permission" below. Populated strictly from the tool input's `question` field; the optional `header` chip-label is never accepted as a substitute. The schema marks `question` `required`, but tool-call schemas are advisory (the model can still omit it — that was the "(no question text)" bug), so the engine enforces it: a batch with any entry missing a non-empty `question` is rejected before any `UserQuestionAsked` is emitted, bouncing it back to the model to re-ask. This field is therefore never empty / `(no question text)`. |
| `options` | `Vec<QuestionOption>` (default `[]`) | Each option is `{ id, label, description? }`. Empty for free-text-only prompts. **There is no text-entry option kind**: picking an option resolves to its `label`, so an agent-authored "Other, I'll type it" row hands that literal phrase back as the answer. The card names both real escapes itself (typing in the prompt, which arrives as a `FreeText` answer, and Cancel, which arrives as `Canceled`), and every question tool description forbids authoring an "Other" option. |
| `worktree_path` | `Option<String>` | The CC worktree at intercept. Required for `--resume` to find the session JSONL (CC keys session storage by CWD). `None` for chat-channel questions and when the request came in without a worktree context. |
| `multi_select` | `bool` (default `false`) | `true` when the user may pick multiple options. |

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
- `{ "kind": "FreeText", "text": "..." }`
- `{ "kind": "MultiSelected", "option_ids": [...], "text": "..."? }`
- `{ "kind": "Canceled" }` — user closed the question without picking
- `{ "kind": "Superseded" }`: a follow-up arrived that could not be the answer, so it replaced the question

**Why `Superseded` exists.** The coding agent is parked *inside* the call that asked, and only an answer makes that call return. So a follow-up that cannot be routed as the answer has to resolve the question anyway. That releases the agent, which then reaches a turn boundary and reads the follow-up. Leaving it open deadlocked the thread. The follow-up's own `CodingAgentPromptSent` killed the card's buttons and the typed-answer route, so nobody could answer it.

Two shapes qualify, both coding-agent-lane only. An agent-driven message (a parent's instruction, a child-completion wake) is refused by the `mode == Human` guard. So is any message landing on a question already overtaken. The tool result tells the agent its question was replaced. It also says the replacement arrives as the agent's next input.

The card reads "Replaced by your next message", never "Canceled". Like `Canceled`, a supersede emits no resume marker and no `ContinuationRequested`. The follow-up drives the next turn itself. Full reasoning and the rejected alternatives: ADR 0082.

**Only the engine writes it.** `POST /api/v1/threads/{thread_id}/answer-question` refuses a `Superseded` body with a 400 and leaves the question pending. The kind asserts that a follow-up arrived and replaced the question, which only the message router can know. Use `Canceled` to dismiss a card, or just send the follow-up.

**An engine restart alone does NOT orphan a pending question.** A thread whose newest event is an unanswered `UserQuestionAsked` is a *preserved checkpoint*: every teardown and recovery path consults one shared predicate (`agent_recovery::thread_has_unanswered_question`) and leaves it strictly alone. No boundary `ResponseAborted`, no synthetic `CodingAgentIdled`, no Continue button, and specifically no graceful interrupt to the subprocess (Claude Code's Esc would cancel the pending `AskUserQuestion` and make the agent race past it). The card stays answerable across the restart, and answering resumes the thread through `ContinuationRequested` then `--resume`. "Preserved" holds only while the question really is the last thing that happened: anything in `ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES` landing after it means the agent moved on, the card is dead, and the thread recovers as an ordinary interrupted turn (abort plus Continue) instead.

**Answering a question the subprocess is no longer waiting for.** Answering normally travels *in band*: the engine wakes the blocked PreToolUse hook (Claude Code) or MCP call (Codex) and the answer returns as that tool's result. When the subprocess was torn down while the card was on screen there is no blocked call left to wake, so the answer emits `ContinuationRequested { reason: "answered_after_idle" }` and the resume message carries the answer itself (`agent_recovery::continue_input_for_reason` over `agent_question::answered_question_recap`): the question text, the answer, and whether the user picked an option or typed their own reply.

The resume cannot instead rely on the resumed agent re-running its hook. On teardown Claude Code closes the pending `AskUserQuestion` in its OWN session transcript with a `tool_result` reading *"The user doesn't want to proceed with this tool use. The tool use was rejected... STOP what you are doing"* (`toolDenialKind: "user-rejected"`, `interruptedByShutdown: true`). That text lives inside the CC binary and inside CC's private JSONL, so it is outside the engine's reach, and because the tool call is closed the hook never re-fires on `--resume`. The engine's own event stream stays clean either way (that is the preserve contract above); what the resume message adds is the other half, telling the agent in words that an interrupted or rejected question call is a teardown artifact rather than a user decision: **the user declined nothing, and approved nothing.** Without it the resumed model reads "card closed" plus a bare "continue" as consent (2026-08-10: it announced *"you declined the card and said continue, so I am treating that as approval"* and ran `lucidos planned approve` against a plan the user had not approved).

**Cancelling one is the mirror case, and it ends the turn.** The cancel stamps the card `Canceled`, and that `UserQuestionAnswered` moves the projection to `running`. With no agent left to interrupt, the Stop handler's settle fallback writes the terminal itself: `ResponseCanceled { user_stop }`. That is what a live agent's interrupt emits, so a Cancel does not read differently because a subprocess survived. `ResponseAborted { stale_settle }` stays for a `running` row nothing in the request explains. Apply / Discard / Archive keep it too, each carrying its own terminator.

**Pairing guarantee.** A `UserQuestionAnswered` is emitted exactly once per `UserQuestionAsked` it answers, enforced by a unique DB index on `(thread_id, tool_use_id)`. The pair can be left unbalanced in real life:

- The agent process dies before the user answers, the engine restarts, and the question is never revisited from the coding-agent side. The `Asked` row stays in the DB without a matching `Answered`. There is **no engine-side timeout that auto-emits `Answered`** for the AskUserQuestion path. (The MCP permission path, `CodingAgentPermissionRequest`, does have an orphan-recovery sweep that emits a `CodingAgentPermissionResolved { allowed: false, reason: "Coding agent terminated before answering — request expired" }` — but that's a different event family.)
- The user closes the popover via the cancel control — that fires `UserQuestionAnswered { answer: { kind: "Canceled" } }`, which still counts as a pair.
- A follow-up lands that cannot be the answer. That fires `UserQuestionAnswered { answer: { kind: "Superseded" } }`, which is a pair too, so watch for both kinds before concluding a question went unanswered.

So a workspace listening for `UserQuestionAsked → UserQuestionAnswered` should treat a long-pending `Asked` with no matching `Answered` as "the user is still being prompted" rather than as a guaranteed-future event.

### `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`

Used by the MCP permission-prompt subprocess (the `mcp__permission-prompt` tool family). Distinct from `UserQuestionAsked`/`UserQuestionAnswered` — see the next section for why both exist.

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

`persist_scope` is `"narrow"` / `"broad"` / `"session"` when the user clicked "Always allow"-style; `null` for plain Allow-once / Deny / orphan-recovery / supersession / session-ended clear. The frontend reads it to render the answered card with a check on the chosen button and strike-through on the rest.

**The Codex shapes.** Codex raises the same two events through the app-server approval bridge, under its own item names. A `command_execution` carries `{command, cwd, reason?}` and summarizes as `command_execution <command>`. A `file_change` is the awkward one: `item/fileChange/requestApproval` carries only `itemId` / `threadId` / `turnId` / `startedAtMs` plus a nullable `reason` and `grantRoot`, and **no paths at all** (both nullable fields arrive `null` in practice). The paths come from the `item/started` notification codex sends for the same item id just before the approval, which the app-server driver copies onto the input, stripped to `{path, kind}` so a multi-file patch's inline diffs never reach the event store:

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

`summary` names up to three paths and then appends `+N more`. A trigger condition matching on it should key off `tool_name` and `input.changes[].path`, not the prose. If the `changes` list was never announced the `changes` key is absent and the summary falls back to `reason` / `grant_root` / the bare tool name, which is what the card degrades to as well.

**Supersession.** If the user types a new message while a permission card is still pending, the engine resolves the card as `allowed: false` (`reason: "Superseded by a new message"`) before routing the typed text to the coding agent as a normal follow-up — so the buttons stop dangling instead of leaving the thread stuck on `waiting_for_user_answer` while the coding agent moves on. This mirrors the AskUserQuestion free-form path, where typing becomes a `FreeText` answer; permissions have no "answer", so the pending request is denied instead. Emitted from `resolve_pending_permissions_as_superseded` (`engine/cc_permission.rs`).

**A parent's `follow_up_child_thread` takes the same path**, and that is the one
side effect of a redirect that is invisible from the verb. Sending a child a
follow-up resolves any permission card pending on it as superseded, so a request
a human was about to approve is cancelled by the redirect. Worth weighing before
steering a child you know is parked on one. Two related facts about following up
a coding-agent child:

- A child parked on a **question** is blocked on a human, not on work. Your
  message is not an answer to that question and is not read until a human
  answers it, and `urgent` does not change that.
- A follow-up that RACES the child's own finish can produce a completion card
  for the turn you interrupted. That does not mean the redirect failed: the
  redirected turn reports separately when it ends.

For everything else about follow-ups (queued versus urgent, what urgency costs,
why it is a no-op on Codex, and that it consumes no child slot) see the *child
follow-up* and *urgent follow-up* entries in `system-knowhow/glossary.md`.

**Session-ended clear + non-resurrecting resolution.** A permission card is answered by unblocking an **in-memory** broadcast waiter — it never spawns a resume (unlike `UserQuestionAnswered`, which can resume an already-idled thread). Two consequences: (1) when a coding-agent session **idles** with a card still pending (a workflow whose parallel subagent's card outlived the main turn, a canceled turn), `emit_coding_agent_idled` clears it via `resolve_pending_permissions_as_session_ended` (`reason: "Coding agent session ended before answering — request expired"`), so a finished thread leaves no dangling clickable card; and (2) the projection flips a thread to `running` on `CodingAgentPermissionResolved` **only from `waiting_for_user_answer`** — a resolution on an already-idle/terminal thread (a stale click hours after the session ended, or the session-ended clear itself) leaves the status unchanged. Before this, tapping such a stale card flipped the sessionless thread to a dead `running` that only a restart's settle sweep recovered (`docs/plans/2026-07-02-cc-permission-card-zombie-running.md`). The same non-resurrecting rule applies to the chat `CommandPermissionResolved` / `McpPermissionResolved` lanes (shared projection arm).

**Resume after an engine restart is NOT a rejection.** When the engine restarts while a session is blocked on a permission card, the in-memory broadcast channel closes, so the still-awaiting `prompt_coding_agent_permission` call returns `allowed: false` with a **neutral** reason (`RESTART_INTERRUPT_REASON` — "Interrupted by an engine restart — not a user decision…"), NOT `DENIAL_REASON` ("User denied"), which is reserved for an explicit Deny click (incl. supersession). That neutral reason is only the MCP / app-server response the agent reads on resume; it is distinct from the persisted `CodingAgentPermissionResolved` reasons above (orphan-recovery / supersession), which are unchanged. On the next turn the session is relaunched with `--resume`, replaying its own transcript, so the recovery system prompts (`recovery_system_prompt` and siblings in `engine/agent_session/prompts.rs`) carry a **restart-not-rejection note** telling the agent that any denial or interrupted/incomplete tool call in its recent history is a restart artifact, not the user rejecting its approach — without it the resumed agent reads the interruption as a rejection and abandons its plan.

**Unattended auto-resolution (no card, never hangs).** A permission card needs a human to click it. A coding-agent thread launched by a **trigger** has none, so before rendering a card the engine asks "is anyone here?". It walks the spawn tree from this thread up to its root, via the persisted `MessageOrigin` chain (`cc_permission::resolve_attend_mode`). It hops only where the thread also carries the `parent_thread_id` callback linkage. A *top-thread* names its *spawning thread* for display but is not in its privilege tree, so the walk stops there.

If the root is a **human device**, the session is *interactive* and behaves exactly as above: emit a card, wait indefinitely. If the root is a **trigger/scheduler**, the thread is *unattended*, either directly trigger-fired or an agent-spawned sub-thread of one. The engine then resolves the request from the originating trigger's *side-effect grant* (see `triggers.md` § "Side-effect grant") plus a static benign check:

- **benign in-workspace work** (a read, an in-workspace write/edit, git, `lucidos data write` to `data/`) → auto-**allow**;
- an **irreversible side-effect** (email / external API / cloud CLI / out-of-workspace destruction / other) whose category is in the trigger's grant → auto-**allow**; not in the grant → auto-**deny** (the agent receives the denial and routes around it or reports the step failed — unlike the chat command guard, this denies the single request, it does **not** fail the whole session);
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

**An auto-ALLOW emits no events** (the same silent fast path session-allow uses). **An auto-DENY emits the ordinary `CodingAgentPermissionRequest` + `CodingAgentPermissionResolved` pair**, back to back, with the command redacted the same way the card path redacts it. Without it the agent reports a failed step and nothing in the timeline says what the engine refused or why. The pair renders as an already-answered card, so it never parks the thread: a trigger thread must not be left needing attention nobody is there to give.

**A trigger subscribed to `CodingAgentPermissionRequest` therefore fires for these too**, on a request the engine already answered. Pair it with the resolution, which lands in the same turn, to tell an engine-answered deny from a card a human still owes.

A **file write is judged over its whole target set**, which matters for Codex, whose `file_change` can name several files in one approval: *any* target outside the workspace root makes the request out-of-workspace destruction (grant-gated), and only an all-in-workspace set is benign. A Codex `file_change` whose paths could not be determined is grant-gated too, since codex raises that approval precisely because the patch escaped its sandbox. Before the driver started attaching the `changes` list, `grant_root` was the only path key such a request ever carried and it arrives `null`, so every out-of-workspace Codex write classified as benign and an unattended session auto-allowed it.

This covers both backends (CC's MCP path and the Codex app-server bridge funnel through one engine function, `prompt_coding_agent_permission`). A user-rooted tree stays interactive even when an agent spawned the leaf coding-agent thread, so a human watching can still answer. Classification is static-only (reuses the *command guard*'s `static_classify` / `fallback_classify`; no LLM judge in the permission path), and the whole decision derives from already-persisted events plus the in-memory trigger registry, so it survives an engine restart. See `cc_permission::{resolve_attend_mode, classify_coding_agent_request}` and ADR 0002 (Phase 5 addendum).

**In-worktree writes never render a card.** Claude Code auto-approves in-cwd writes under `--permission-mode acceptEdits` **except** under `.claude/` and `.git/`, which it routes through the permission-prompt tool in every mode and regardless of `--allowedTools`. Because Lucidos keeps all of its own agent configuration in `.claude/`, that made editing a rule or a skill cost a click on every save — and the persisted "Always allow" scopes can't suppress it, which is why the broad button is hidden on those cards. So the engine answers first: a **file write whose target resolves inside the session's own worktree** is auto-allowed with no card and no events (the same silent fast path session-allow and the unattended path use). It is safe because the worktree is disposable and every change in it is reviewed in the Diff before Apply. Containment is **resolved against the real filesystem, not matched lexically** — both the worktree root and the longest existing prefix of the target are canonicalized — so a symlink inside the worktree pointing somewhere external can't launder an outside write past the card. Four deliberate limits: a `..` component fails containment; a relative path fails closed (resolving one needs the agent's cwd, which is the worktree for a repo-rooted spawn but `data/apps/<id>` beneath it for an app coding-agent thread — and both backends send absolute paths anyway); any `.git` path component still renders a card, checked before *and* after symlink resolution (git metadata is the one in-worktree location that is *not* in the reviewed diff — a written hook would run on the next commit); and commands (`Bash` / `command_execution`) are never covered, so a shell command touching `.claude/` still asks. A write **outside** the worktree — the user's global `~/.claude/settings.json`, say — is unchanged and still asks. See `cc_permission::{worktree_write_auto_allowed, path_inside_worktree}`.

**Two folders join the worktree as *working directories*.** The engine grants them in `cc-settings.json`, so reaching either raises no card: the workspace's `data/` tree, and the OS temp dir. That narrows the paragraph above, whose "outside the worktree" predates both. The mode it names is a preference now, the *coding-agent permission mode*. Under `auto`, Claude Code's own classifier decides instead of carding.

**"Allow for this thread" survives an engine restart.** The grant is durable: it is persisted as the resolution's `persist_scope: "session"`, and `cc_permission::hydrate_session_allows` refills the thread's in-memory set from the event store on the first prompt after a restart (re-deriving each pattern through the same `derive_allow_pattern` the grant used, so the two sites can't drift). Without it, an Apply-with-restart silently dropped every grant the user had clicked while the thread itself resumed, and the same file re-asked minutes later. Only `allowed: true` **and** `persist_scope: "session"` rehydrate — an Allow-once, a Deny, the `narrow`/`broad` scopes (which live in `cc-allowed-tools` instead), and the engine's own supersession / session-ended / orphan-recovery resolutions all do not.

**"Always allow" binds the session it was clicked in.** Both persisted scopes append a pattern to this workspace's `cc-allowed-tools`, which Claude Code reads as `--allowedTools` at spawn. That flag is frozen for the subprocess's life. So a click took effect one session late, and the tool carded again seconds later. The engine's gate now reads the file itself, on every prompt. The grant covers the running session, and every other live thread in the workspace.

It honours a stored pattern only where `derive_allow_pattern` would have produced it, which is the codebase's own record of what CC respects. A bare `Edit` / `Write` / `NotebookEdit` / `ExitPlanMode` line covers nothing. A `Bash` command touching `.claude/` or `.git/` still cards, and the Codex tools are untouched. The gate sits BELOW the unattended one, so a workspace grant never answers for a trigger-rooted session. A catastrophic command is still denied, whatever the file says. See `cc_permission::persisted_allow_covers` and ADR 0125.

## Question vs. permission — and why both event families exist

There are four distinct user-facing prompt mechanisms in play, riding on two event lanes:

1. **CC's native `AskUserQuestion` tool.** A first-class tool CC can call to get structured input mid-turn ("which approach", "is this OK to do"). Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = claude_code`. The `question` payload field is the literal prompt text CC supplied.
2. **Codex's `ask_user_question` MCP tool.** Same wire shape, same QuestionCard rendering, raised from a Codex session via the `lucidos` MCP server (`lucidos mcp-permission-server`). One question per call; the MCP server long-polls the same internal endpoint CC's hook uses and returns the user's answer as the MCP tool result, so the Codex turn continues in place. Fires with `meta.channel = claude_code` (the coding-agent channel). **Codex-only, and enforced as such**: CC spawns the same binary with `--permission-only`, so this tool is not in a CC session's tool list at all. It used to be, and the duplicate cost a click on every question (CC routes every MCP tool call through its permission prompt) plus a redundant tool-call step beside the card.
3. **Chat agent's `ask_user_question` tool.** Same wire shape, same QuestionCard rendering, but raised by the in-process chat agent rather than a coding-agent subprocess. Fires `UserQuestionAsked` / `UserQuestionAnswered` with `meta.channel = chat`. The chat tool blocks on the question wait registry until the answer arrives, then returns the joined `{question_text: label}` map as the tool result on the same turn. Use this from chat threads whenever a button-driven answer beats forcing the user to type (see `crates/lucidos-engine/src/llm/tools/misc.rs` § `ask_user_question_tools`).
4. **Coding-agent permission prompts.** Authorization for a specific tool call, possibly with persistence ("Always allow Bash(git:*)"). Fires `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`. Two raise paths into the same engine machinery: CC's MCP permission-prompt subprocess (consulted whenever CC would invoke `Edit`/`Write`/`Bash` on a path it doesn't have a static allow rule for) and the Codex app-server approval bridge (`item/commandExecution/requestApproval` / `item/fileChange/requestApproval` JSON-RPC requests under `approvalPolicy: on-request`).

These look similar in the UI (both render as a card the user has to act on) but they are two separate event lanes. **A "permission prompt" is not a third event family**; it's whichever of these two raised it.

In practice today:

- A workspace LLM that says "notify me when a coding agent is asking for permission" almost always means *either*: it should subscribe to `UserQuestionAsked` (covers the AskUserQuestion case and any permission-style prompt that the engine routes through the question registry), *or* — for true permission prompts — `CodingAgentPermissionRequest`. Pick based on what the user actually wants to act on.
- `CodingAgentPermissionRequest` is also triggerable — both blocking-request paths flow through the matcher, so you can pick whichever matches your case (or wire a separate trigger to each). Note it fires **only on interactive (human-rooted) sessions**; an unattended trigger-rooted coding-agent thread auto-resolves its requests silently (no event — see "Unattended auto-resolution" above), so a "notify me when a coding agent asks permission" trigger won't fire for unattended runs (by design — there's no one to notify, and the run doesn't stall).
- Lucidos does NOT seed a default question-push trigger. Workspaces opt in by creating a user trigger themselves (see the worked example in `triggers.md` § "Worked example: push when agent needs me").

## The error gap

There is no `CodingAgentErrored`, `CodingAgentFailed`, or `CodingAgentCrashed` event. Verified via `rg -n "CodingAgent(Error|Errored|Failed|Crashed|Aborted|Canceled)" crates/lucidos-engine/src/` — zero matches.

When a coding-agent session fails mid-turn (upstream API error, OOM-killed bash, empty assistant text on a non-cancel turn), the engine routes it through `classify_result` (`agent_session/lifecycle.rs`) and:

- emits a chat-side `ResponseFailed { error }` to mark the turn as failed (sets the thread's projected `status = 'failed'` → the red error dot in the thread list);
- emits `CodingAgentIdled { has_changes: <whatever was on the branch> }` so the dispatcher closes the turn and the UI exits the "Working" state. This bookkeeping idle deliberately does NOT downgrade the `failed` status back to `idle`.

**A turn's verdict is sticky.** There are two. `failed` means the turn errored, or was interrupted with nobody coming back for it. `paused` means the user's own version switch interrupted it and the engine is resuming it, see the next paragraph.

Only a real *start* event clears one (`MessageReceived`, `CodingAgentUserMessageSent`, `UserPromptInjected`, `CodingAgentPromptSent`, `ContinuationRequested`), because only those mean new work was actually requested. Every event that merely *closes out* the ended turn routes its status write through the shared `preserving_verdict` guard (`engine/event_bus/mod.rs`) and leaves the verdict alone: the trailing activity stream, `ChangeProposed`, `CodingAgentIdled`, `SessionEnded`, and `ResponseCanceled`. So the status indicator persists until the user sends a follow-up or clicks Continue (the red dot for `failed`, the pause glyph for `paused`).

**`paused` is a promise of an auto-resume, and only that.** `AbortCause::status_sql()` splits on `AbortCause::promises_auto_resume()`, which reads the abort's ACTOR as well as its cause. A `ResponseAborted` carrying `EngineShutdown` **and** a device actor is the *Switch to new version* teardown. That is the one interruption the engine brings back by itself, and it settles the thread at `paused`. `StaleSettle` keeps the cancel-style idle mapping.

Everything else settles at the red `failed`, because nobody is coming back for it. That covers `SafetyNet`, `ProcessKilled` and `SessionDropped`. It covers a system-actor `EngineShutdown`, the shutdown fallback for a thread that started after the restart pre-emit. It covers `RecoveryAfterRestart` in both roles: the boot sweep's crash boundary, and the boot floor *withdrawing* a promise it could not keep.

**A pending change does not change that answer.** Both verdicts used to defer to one, writing `waiting` instead. The ordering behind that (a change to review outranks the interruption) is right and is kept, but it belongs to the reader: the thread list ranks `paused` and `failed` above the changes dot on its own, and `coding_agent_proposed` is what says a change is there. Writing it into the status cost the verdict outright, because `waiting` is not one of the two values `preserving_verdict` protects. The dying subprocess's drain landed milliseconds later and wrote `running` over it, so an interrupted thread carrying a change came back reading Running. It also hid such a thread from the boot floor above, which selects on `status = 'paused'`.

The split is deliberately this narrow. It keyed on `AbortCause::is_transient()` until 2026-08-06, which has no actor axis, so a crash and a withdrawn promise both wore the reassuring pause glyph and stayed out of the needs-attention count. The same predicate withholds the **Continue** button on the frontend (`abortPromisesAutoResume`), so the two now cannot disagree: a paused thread never offers Continue, and a thread offering Continue never reads paused.

That guard matters far more for a coding agent than for the *Lucidos Agent*, and the two channels visibly disagreed until it was applied uniformly. A chat turn's loop emits nothing after its own terminator. A coding-agent turn emits four more things, because the subprocess outlives the terminal event.

An interruption (engine restart, watchdog kill) emits `ResponseAborted` while the agent is still alive. `external_terminal_emitted` suppresses only the duplicate *terminal*, not the drain, so the final `CodingAgentTextStreamed` / `CodingAgentToolResult` land milliseconds later. Then comes the `ChangeProposed` the agent commits on its way out, then `CodingAgentIdled` and `SessionEnded`. Before the guard covered all four, that trailing traffic walked an interrupted coding-agent thread from `failed` back to `idle` and the dot vanished. An interrupted Lucidos Agent thread kept it. Backend-agnostic: the drain lives in the shared `agent_session` layer, so Claude Code and Codex behave identically.

`ResponseFailed` and `CodingAgentIdled` are both triggerable today. The cleanest wiring for "notify me when a coding agent errors" is:

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

For a workspace-defined failure name, `lucidos events emit CodingAgentFailureObserved {...}` (or the `emit_event` LLM tool) writes a `SystemEvent::DomainEvent` that flows through the matcher unconditionally. Use this when the engine's own ThreadEvents don't carry the discriminator you need (e.g. you want to distinguish OOM from API-503 in the trigger).

## Notes on `CodingAgentPromptSent`

This is NOT the user typing into a coding agent. User-typed input fires `CodingAgentUserMessageSent`. `CodingAgentPromptSent` is for prompts the engine itself synthesized:

- merge-conflict explanation injected after `MergeConflictDetected`,
- `MissingHardeningDetected` recovery prompt asking the coding agent to run `/harden`,
- the empty `CodingAgentPromptSent` emitted right after `UserQuestionAnswered` so the timeline shows a "thinking" placeholder while the coding agent processes the answer. Skipped for two answer kinds, both because no turn of its own follows. `Canceled`: the cancel-stamp path (`claude_code_stop`, `archive_thread`) tears the coding agent down right after, so the marker would strand as an empty step. `Superseded`: the follow-up that replaced the question emits its own `CodingAgentPromptSent` moments later, and that is the placeholder. See `emit_resume_marker_for_cc_answer` in `crates/lucidos-engine/src/engine/agent_question.rs`.

Distinguishable on the wire by `origin: Some(MessageOrigin::Engine { reason: ... })`. Workspaces should generally not need to subscribe to this — it's an audit trail event, not a lifecycle signal.

## Resume-time notes prepended to the next prompt

When a coding-agent thread resumes (`--resume` replays the prior conversation but NOT what changed on disk or in `main` since), the engine reconciles the gap by prepending up to three short `[Note from engine: …]` blocks to the user's next message. These are NOT persisted events. They ride only on the in-memory prompt text handed to the agent, assembled once in `build_resume_prompt_text` (the single injection point for these three, for both Claude Code and Codex), and only when the user's message is non-empty:

- **Branch adoption**: the worktree was switched to a new branch holding the agent's prior work; the engine adopts it and says so.
- **Turn gap**: what the user or the engine did to the agent's work while it was idle. See below.
- **External edits**: the worktree changed between turns, detected by diffing it against the `worktree_head_sha` recorded on the last `CodingAgentIdled`. The wording names no cause. The detector sees a SHA and a `git status`, so it cannot tell a hand edit from an engine reset, and it must not blame the user for one. When the turn-gap note already states the cause of a HEAD move, exactly one line is suppressed: the `HEAD moved (no log available)` fallback, which is the signature of a backwards reset. A real commit log and any uncommitted change are still reported.

### The turn-gap note

The **turn gap** is the window between the agent's previous turn boundary and the current one. Events that land there are invisible to the resumed agent, because `--resume` replays its own conversation and nothing else. Left unsaid, the agent tells the user a discarded change is awaiting Apply, offers to Apply commits that no longer exist on the branch, or treats reverted work as still in `main`.

| Event in the gap | What the note tells the agent |
|---|---|
| `ChangeApplied` | Merged into `main` and the worktree reset to match. Lists the merged commit subjects with the short post-merge `main` SHA. Not pending anymore. |
| `ChangeDiscarded` | Discard reset the change's branch to `main` and cleaned the worktree, so those commits are gone. Names the branch, and says whether it is the session's own branch or a stale change on a different one (the reconcile path discards siblings on other branches, and that must not read as "your work is gone"). Not pending: do not offer to Apply it. |
| `ChangeReverted` | The change had applied and has since been undone in `main` by revert commits, so `main` no longer contains that work. The branch and worktree were not touched, because a revert runs in the main repo. |
| `ChangeApplyFailed` | An Apply attempt did not land and the change is still pending. The engine's error is quoted and attributed as the message shown to the user, never rendered as an instruction to the agent. |
| `WorktreeCleaned` | The cleanup worker reclaimed the worktree. Tier 1 stripped `target/` / `node_modules/` / `.lucidos/cache/`, so the next build starts cold; tier 2 removed and recreated the whole worktree, so untracked files are gone, and may have deleted a fully-merged branch. |

Deliberately NOT in the note because another mechanism already delivers them: `ChildThreadCompleted` (itself a coding-agent turn origin, it wakes the parent carrying the child's summary), `CodingAgentPermissionResolved` (delivered in-band to the waiting call), `UserQuestionAnswered` (delivered in-band when the subprocess is alive, and as the `answered_after_idle` resume message's own BODY when it is not, see "Answering a question the subprocess is no longer waiting for" above; that body carries the same `[Note from engine: …]` marker but is not one of the three reconciliation notes here, and is assembled in `agent_recovery::continue_input_for_reason` rather than in `build_resume_prompt_text`), and `BackgroundBashCompleted` (its completion watcher pushes its own resume prompt). Deliberately NOT in the note because it would add nothing: `MergeConflictDetected` and the merge-resolution events (the conflict session spawns with a purpose-built system prompt naming the conflicted files), `ChangeHardened` (the agent's own `/harden` caused it), `CodingAgentSettingsChanged` (the resumed process already runs with the new model), the cosmetic thread events, and `ThreadArchived` / `ThreadDiscarded` (terminal, and archive's pending-change discard already arrives as `ChangeDiscarded`).

The note is **stateless and self-clearing**: it surfaces every covered event whose `sequence` falls between the *previous* turn boundary and the *current* turn's triggering event. The current turn's origin is persisted before the agent is resumed, and the window is bounded by its `sequence`, which is what makes the threshold the previous boundary. Anything that lands after the origin (a second message racing the same spawn, a click while the agent is starting) belongs to the next turn's gap and is surfaced there. The boundary set is every event type that can originate a coding-agent turn (`MessageReceived`, `CodingAgentUserMessageSent`, `TriggerStarted`, `ChildThreadCompleted`) plus `CodingAgentPromptSent` for engine-synthesized prompts. Because every turn has one, this turn's origin becomes the next turn's boundary, so an event is surfaced exactly once and never re-fires. No new event, projection column, or migration backs it; the `events` table is the cursor.

## Recipe-shaped guidance

**Every recipe below is a *trigger*, which is the right shape only when the reaction should outlive the conversation and reach the user as a notification.** If the user asked to be told **in the thread they are typing in** ("let me know here when a coding agent edits code"), that is the `await_event` tool, not a trigger: a trigger runs in its own thread and cannot report into an existing conversation. The subscription shape is identical, so the `on` entries below transplant verbatim into an `await_event` call. Pick the mechanism by where the answer has to land, then by how long it has to last. See `system-knowhow/triggers.md` § "When a trigger is the right answer".

For the trigger config field reference (cron format, the `on` subscription list, per-entry `condition` operators), see `system-knowhow/triggers.md`. A condition key is a field path: a dot reads one level down. Operators: `$eq` `$ne` `$lt` `$lte` `$gt` `$gte` `$in` `$nin` `$regex` (a bare value is `$eq`), plus `$or` in key position taking a list of conditions. See `system-knowhow/triggers.md` § "What a condition can say" for the full language.

### Notify when a coding agent is waiting on the user

```yaml
on:
  - event_type: UserQuestionAsked
run:
  intent: "Notify me when the coding agent has a question waiting for me. The push should deep-link straight to the question — tapping it takes me to the originating thread and pulses the question card on land."
```

The `tap` + `event_id` combination is what makes the push actually deep-link: the push tap navigates straight to the originating thread *and* the matching event card pulses on land. Without them the push defaults to opening the inbox modal. See `triggers.md` for the full pattern.

To scope to a specific coding-agent session, add a per-entry `condition`:

```yaml
on:
  - event_type: UserQuestionAsked
    condition:
      cc_session_id: "abc123-…"
```

Conditions are pure payload filters: they look at the event's own payload fields, plus `thread_id`, which the engine supplies for every thread event. `cc_session_id` is on `UserQuestionAsked`'s payload (see shape above), so this works, and `{ "thread_id": "<uuid>" }` works on any thread event. Nothing else about the thread is reachable from a condition (no title, no app id, no status), and a **domain event** has no `thread_id` either, since it belongs to no thread.

### Notify when a coding-agent session finishes / produced changes

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      has_changes: true
run:
  intent: "Tell me the coding agent finished and left a change to review."
```

Adding `is_external_repo: { $ne: true }` scopes to the engine repo; `requires_restart: true` scopes to changes that need an Apply & Restart, etc. — same condition language across all per-payload fields.

### Wait for one named coding-agent session to finish

```yaml
on:
  - event_type: CodingAgentIdled
    condition:
      thread_id: "<uuid>"
```

The same entry shape works as an `await_event` subscription, which is how a chat thread waits for a coding-agent session it did not spawn: list the running ones (`threads` list, `status: ["running"]`), then subscribe per session. Two things to know before relying on it. `CodingAgentIdled` is a **turn boundary**, not a session terminator, so a session that gets a follow-up emits another one later; and a subscription is spent by the first match, so waiting on several sessions means re-listing on each wake and subscribing again while any are still running.

### Notify when a coding agent errors

```yaml
on:
  - event_type: ResponseFailed
run:
  intent: "Send me a push notification with the failure error."
```

`ResponseFailed` fires for both chat and coding-agent failures — see "The error gap" above for what "error" means in this codebase and the alternatives if you need finer discrimination.
