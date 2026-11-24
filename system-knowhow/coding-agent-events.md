---
name: Coding Agent Lifecycle Events
description: ThreadEvents a Claude Code or Codex session emits, which ones the scheduler blocks, and how to spawn one with `run_coding_agent`. Load for "notify me when a coding agent finishes / asks a question / errors", "which folder should the coding agent edit", "wait until the agent is idle", "watch for a permission prompt", "CC session", "AskUserQuestion".
---

<!-- domain-event: CodingAgentFailureObserved: The worked failure-monitoring script emits this domain event. -->

# Coding Agent Lifecycle Events

`CodingAgent*` is the engine's umbrella name for events a coding-agent session emits, Claude Code or Codex. Both backends translate their CLI output into the same `ThreadEvent` enum. A `coding_agent: CodingAgent` field tells them apart. It defaults to `ClaudeCode` on legacy DB rows, and `#[serde(alias = "agent")]` decodes rows persisted before the rename.

The session's *worktree* wraps a git repo that depends on `coding_agent_kind`:

- `lucidos` (default): a full checkout of the Lucidos source repo. It exists only when the engine was launched from a Lucidos source checkout. A packaged install ships the binary alone, so the compose destination picker hides the "Lucidos source" target and `run_coding_agent` with `folder` omitted is refused. An `.app` bundle remains packaged even when built inside a checkout; it cannot discover platform source above the bundle or through its compile-time build path.
- `app`: a sparse checkout of the workspace git, narrowed to `data/apps/<id>/` (see *app worktree*). Apply ff-merges to the workspace git's local `main`, with no engine restart and no `/harden`. `AppUiRefreshRequested` emits when iframe-bundled files change.
- `external`: a full checkout of the user-registered external git repository. No Apply / Discard surface.

`coding_agent_kind` ships on every `SessionStarted` event and persists in `thread_summaries.coding_agent_kind`, so the apply path dispatches without re-reading the event log.

### Choosing `folder` when you spawn one

`run_coding_agent`'s `folder` argument selects the kind above, so pick it before
you call. Ambiguous? Ask which folder first.

The `coding_agent` argument picks Claude Code or Codex. Omit it to run the installed agent, Claude Code first. Naming an agent that is not installed is refused, and the refusal names the one that is. A spawn into another workspace (`workspace`) skips this check: this engine cannot see that host's agents, so an omitted `coding_agent` means Claude Code there.

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
session lands on that engine. The call goes through the gateway, which vouches
for this thread and its coding-agent check, so the target honors that check.
When no gateway can vouch (none serves the target, and no paired one does), the
tool posts directly. A coding agent there then waits for the owner's **Allow
once**. It requires
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

**Backend selection (Claude Code vs Codex).** The thread's FIRST send picks the backend: the compose destination picker's coding-agent chip sets `coding_agent` on the chat request. The default is `claude-code`, remembered per workspace via the `coding_agent_default` preference. The value ships on `SessionStarted.coding_agent` and persists in `thread_summaries.coding_agent`. It is **locked within a session**: follow-ups and recovery always resume on the stored backend, since the other backend has no session to resume. A follow-up requesting a different backend gets `409 Conflict`.

Only a *successor session* may switch it, through `lucidos successor start --coding-agent`, and `thread_summaries.coding_agent` then follows the newest `SessionStarted`.

Differences a workspace can observe:

- Codex emits coarse tool events (`command_execution`, `file_change`, `mcp_tool_call`, `web_search`, `todo_list`) instead of CC's named tools (`Bash`, `Read`, `Edit`, …). Trigger conditions on `CodingAgentToolCalled.name` must match per backend.
- `UserQuestionAsked` / `UserQuestionAnswered` fire for **both** backends. CC routes through its `AskUserQuestion` PreToolUse hook. Codex calls the `ask_user_question` tool on the `lucidos` MCP server (one question per call), which hits the same blocking engine endpoint. The answer returns inside the same Codex turn as the MCP tool result. Neither backend emits a `CodingAgentToolCalled` for the question tool (`AskUserQuestion` and `mcp__lucidos__ask_user_question` are suppressed), so wire triggers to `UserQuestionAsked`, not to the tool name.
- `CodingAgentPermissionRequest` / `CodingAgentPermissionResolved` always fire for CC. They fire for Codex under the default `app-server` protocol. Inside the agent sandbox Codex runs with `approvalPolicy: untrusted`. Every command then raises the same PermissionCard as a CC `Bash` call, which `coding_agent_auto_answer` can settle, and so does every out-of-worktree file change. With the sandbox off, Codex keeps its own sandbox and `on-request`, so only sandbox-escaping commands ask, as `command_execution` / `file_change`. Under the `LUCIDOS_CODEX_PROTOCOL=exec` escape hatch, Codex runs with the OS sandbox (`--sandbox workspace-write`) as the only guard and emits **no** permission events.
- `/harden`, Apply / Discard, worktrees, branches and every event in this file are backend-agnostic: same lifecycle, same payload shapes.

Two recurring confusions: there is no `CodingAgentPermission*Event` separate from `UserQuestionAsked`, and there is no `CodingAgentErrored` event at all.

For the **full ThreadEvent enum**, with persistence and triggerability for every variant, see `system-knowhow/thread-events.md`. This file is the coding-agent deep-dive.

For trigger config syntax (cron vs the `on` subscription list, per-entry `condition` filters, `run.intent` discipline), see `system-knowhow/triggers.md`. For event-store column shape and the chat-side terminator events (`ResponseGenerated` / `ResponseFailed` / …), see `.claude/rules/db.md`.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/coding-agent-events/triggerability-blocklist-semantics` | Triggerability: blocklist semantics |
| `system-knowhow/coding-agent-events/the-full-enumerated-list` | The full enumerated list (Persisted, low-volume: terminal-state or one-per-turn; Persisted, high-volume: pair with `condition:` (or, for streaming, blocked entirely); Transient: never persisted, broadcast over SSE only; `UserQuestion*`: the question / permission channel); Model swaps |
| `system-knowhow/coding-agent-events/billing-evidence` | Billing evidence; Concrete payload shapes (`CodingAgentIdled`; Side questions are recorded, and hidden from every agent; `UserQuestionAsked`) |
| `system-knowhow/coding-agent-events/concrete-payload-shapes` | Concrete payload shapes (continued) (`UserQuestionAnswered`) |
| `system-knowhow/coding-agent-events/concrete-payload-shapes-2` | Concrete payload shapes (continued) (`CodingAgentPermissionRequest` / `CodingAgentPermissionResolved`) |
| `system-knowhow/coding-agent-events/question-vs-permission-why-both-event-fa` | Question vs. permission: why both event families exist; The error gap; Notes on `CodingAgentPromptSent` |
| `system-knowhow/coding-agent-events/resume-time-notes-prepended-to-the-next` | Resume-time notes prepended to the next prompt (The turn-gap note); Recipe-shaped guidance (Notify when a coding agent is waiting on the user; Notify when a coding-agent session finishes / produced changes; Wait for one named coding-agent session to finish; Notify when a coding agent errors); A human Stop holds automatic wakes |
