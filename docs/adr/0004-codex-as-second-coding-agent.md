# 0004 — Codex integrates as per-turn `codex exec` processes; backend locked per thread; coding-agent channel names stay `claude_code`

- **Status:** Accepted (the §1 per-turn default and the §4 "sandbox is the
  only guard" half are superseded by [0005](0005-codex-app-server-protocol.md);
  the exec model survives as the `LUCIDOS_CODEX_PROTOCOL=exec` escape hatch)
- **Date:** 2026-06-12

## Context

The work-tracker item *Codex as a CodingAgent* called for OpenAI's Codex CLI
as the second `AgentRuntime` — deliberately the "cheapest second-mover", to
surface what belongs in the trait vs what was Claude-Code-specific leakage
before ForgeCode / Cursor follow.

Codex CLI (0.139.0) has no long-lived stdin protocol comparable to CC's
`--input-format stream-json`. Its stable programmatic surface is
`codex exec --json` — exactly ONE turn per process, JSONL events
(`thread.started` / `turn.*` / `item.*`) on stdout, resume via
`codex exec resume <thread_id>`. The experimental `codex app-server` could
host a persistent session, but is explicitly marked experimental and shaped
for IDE integrations.

## Decision

1. **Per-turn process model, not app-server.** `CodexRuntime` presents one
   long-lived `RunningAgent` over a *sequence* of short-lived `codex exec`
   children (`runtime/codex.rs` driver). This matches the engine's own
   session model — the engine already terminates the agent subprocess at
   idle (`IdleAction::ExitSubprocess`) and re-enters via the persisted
   session id on the next message, so "process per turn" is the shape the
   engine assumes anyway. The Codex thread id rides the existing
   `cc_session_id` plumbing unchanged.

2. **Backend chosen at first send, locked forever.** `coding_agent` on the
   chat request (compose-view picker) → `SessionStarted.coding_agent` →
   `thread_summaries.coding_agent` (COALESCE keeps existing). Follow-ups and
   recovery resolve from the stored value; a mismatched request 409s
   (`validate_thread_continuity`). Rationale: the other backend has no
   session to resume — a mid-thread flip silently loses the whole
   conversation. Cross-backend handoff (e.g. "continue this CC thread with
   Codex via reconstructed history") is possible later but is a feature,
   not a default.

3. **Trait changes surfaced by the second mover** (the point of the
   exercise):
   - `SpawnArgs.continuation: bool` — CC's `--print --resume` auto-injects
     "Continue from where you left off."; the trait now carries the
     engine's *intent* so runtimes without that side effect can synthesize
     the prompt.
   - `runtime/spawn_env.rs::apply_lucidos_env` — the agent-independent env
     contract (workspace resolution, host protection, PG*, subprocess
     origin, spawn metadata, RUSTC_WRAPPER gate, lucidos-CLI PATH)
     extracted from CC's `build_command` so no runtime can ship without it.
   - The engine's interrupt arm waits for a `Result` after forwarding
     `ControlRequest::Interrupt`; runtimes whose CLI can't wind down
     gracefully must synthesize one (and close any open tool calls so the
     paired-tool watchdog counter re-arms).

4. **Sandbox instead of permission protocol.** Codex runs
   `--sandbox workspace-write` with network on and the worktree's shared
   git dir granted via `--add-dir` (a linked worktree's `.git` is a file
   pointing into the main repo, so in-agent `git commit` needs it). No
   permission cards, no AskUserQuestion — those stay CC-only features wired
   through CC's PreToolUse hook.

   *Update 2026-07-26 — the workspace `data/` tree is a second writable
   root.* The shared git dir was the only `--add-dir`, so writes to the
   PARENT workspace's `data/` — which is the *documented* contract for a
   coding-agent thread (`lucidos data write` / `lucidos data path` resolve
   under `<workspace>/data/`, and workspace knowhow tells agents to log
   follow-ups to `artifacts/work-tracker/data.json`) — were outside every
   writable root and the macOS seatbelt refused them with `EPERM (os error
   1)`. That silently broke the 2026-07-26 nightly's Codex security pass,
   which lost two high-severity findings; Claude Code runs unsandboxed and
   never hit it, so the contract looked like it worked. `CodexConfig` now
   carries a `sandbox_writable_roots: Vec<PathBuf>` (resolved by
   `codex::sandbox_writable_roots`) holding the git dir **and**
   `<workspace>/data`, emitted as `--add-dir` on exec and
   `sandbox_workspace_write.writable_roots` on app-server. Scoped to
   `data/`, deliberately **not** the workspace root: the root also holds
   `.lucidos/` (engine runtime, logs, gateway registry) and every sibling
   worktree. The sandbox stays the guard for everything else; a new entry in
   that list is a reviewable widening, pinned by tests asserting the set is
   exactly the configured roots.

   *Update 2026-06-12:* both halves have since been closed. AskUserQuestion
   landed as the `ask_user_question` MCP tool (`lucidos
   mcp-permission-server`, hitting the same blocking internal endpoint CC's
   hook uses) — works on every Codex protocol. The "sandbox is the only
   guard" half is superseded by ADR 0005: the default `codex app-server`
   protocol runs `approvalPolicy: on-request` and raises real permission
   cards; the per-turn `codex exec` escape hatch
   (`LUCIDOS_CODEX_PROTOCOL=exec`) keeps the sandbox-as-guard model
   described here.

## Deliberate no's

- **`EventChannel::ClaudeCode` / `source = 'claude_code'` / the
  `claude-code/` branch prefix / `cc_*` field names are NOT renamed.** They
  are load-bearing wire/DB/recovery surfaces (`agent_recovery` scans
  branches by the `claude-code/` prefix; every client filters
  `source = 'claude_code'`). They now mean "coding-agent channel",
  documented as such in both glossaries. A wholesale rename to
  `coding_agent`-rooted names would touch every persisted row, every
  client, and recovery — separate migration-sized change if ever worth it.
- **`spawn_agent_thread` (the chat LLM tool) stays CC-only.** It passes no
  backend; giving the LLM a `coding_agent` arg is deferred until there's a
  reason for the model to pick Codex.
- **No Codex AGENTS.md / skill injection — taught via first-turn
  instructions instead** (resolved 2026-06-12, see
  `docs/plans/2026-06-12-codex-workspace-writes-and-user-questions.md`).
  CC sessions get the lucidos-cli skill installed into the worktree; Codex
  gets a condensed lucidos-CLI + `ask_user_question` section appended to its
  engine-side system prompt (`prompts.rs::append_backend_rules`), delivered
  inline on the first fresh turn. AGENTS.md injection was rejected for two
  hazards: external repos may carry their own AGENTS.md (appending creates a
  dirty diff the agent can commit into the user's repo), and hiding an
  injected file via `info/exclude` leaks into the user's main checkout for
  linked worktrees (the exclude file lives in the shared git dir — same
  class of problem `hide_phantom_tracked_skill` cleans up for CC).
- **Reasoning summaries are dropped.** Codex `reasoning` items have no slot
  in the coding-agent timeline (CC's thinking isn't surfaced either).

## Consequences

- Adding ForgeCode / Cursor is now: enum variant + runtime module +
  registration + menu-options JSON; the engine loop, events, projection,
  apply flow, and recovery are backend-agnostic.
- Codex threads stream chunky (item-level, no per-token deltas) — accepted;
  the engine's flush logic already tolerates arbitrary chunk sizes.
- The `/model` picker for Codex is a hand-maintained list
  (`runtime/codex_menu_options.json`), same maintenance model as CC's
  (`update-lucidos-cc-models` skill covers CC; Codex updates are manual
  until a sibling skill exists).
