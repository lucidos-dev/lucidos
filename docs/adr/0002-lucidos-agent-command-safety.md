# 0002 — Lucidos Agent command safety: gate the dangerous slice, not every command

- **Status:** Accepted
- **Date:** 2026-06-08

## Context

The **Lucidos Agent** (the chat/trigger agent) runs `run_bash` / `run_bash_background`
/ `run_python` / `run_python_background` with **no pre-execution check of any kind**.
A command string flows straight from the LLM tool call to `/bin/sh -c <cmd>` (cwd =
workspace) or to the workspace venv. The only existing guards are the **circuit
breaker** (blocks calling the *same* target 3+ times — useless against a single
`rm -rf`), a **timeout** (kills hangs), and the **Stop button** (kills mid-run).
None of these stop a *destructive* command; they stop a *stuck* one.

By contrast, the **coding agent** (Claude Code session) already has a real
permission model: `--permission-mode acceptEdits` + an MCP permission-prompt-tool
that blocks the subprocess and renders a `PermissionCard`, backed by
`CodingAgentPermissionRequest` / `CodingAgentPermissionResolved` events, an
in-memory `CcPermissionState` (dedup + per-thread `session_allows` + broadcast
fan-out), an `AllowScope` (Narrow / Broad / Session), `derive_allow_pattern`, and a
persisted `~/.lucidos/cc-allowed-tools` allowlist.

The ask: add a safety mechanism for bash/python "screwups" **without forcing the
user to approve every command**. The cost we're avoiding is not *approval* — it's
*approving the safe 95%* (reads, curls, data crunching, in-`data/` writes — all
recoverable or harmless). The damage comes from a thin slice: irreversible
real-world side-effects (mutating `curl`, `osascript`, `mail`, `requests.post`,
spending money), out-of-workspace destruction, and catastrophic local commands.

Confirmed scope from the design dialogue:

- **Threat model:** primarily real-world side-effects (irreversible by nature);
  secondarily out-of-workspace *destruction*. Out-of-workspace **reads/edits/fixes
  are a wanted feature** (Lucidos fixing things on the Mac) — explicitly *not* a
  threat.
- **Blast radius is a risk signal, not a wall.** "Outside the workspace" never
  refuses on its own; it only bumps a command into a higher-risk lane.
- **Triggers** fire unattended — there is nobody to approve — so they get
  pre-authorization at setup instead of a runtime prompt.
- **Classifier** is hybrid: deterministic static rules for the obvious cases, a
  cheap LLM-judge for the ambiguous middle.

## Decision

Add a **command guard**: a pre-dispatch gate in the agentic loop
(`agentic_loop/run.rs`, where the circuit-breaker check sits today) in front of the
four bash/python tools. Every command is sorted into one **risk lane** and handled
accordingly:

| Lane | Examples | Action |
|---|---|---|
| **Safe** | reads, `git status/log/diff`, GET curl, in-`data/` writes, data crunching | run immediately, no gate |
| **Catastrophic** | `rm -rf /`, fork bomb, `mkfs`, `dd of=/dev/*` | hard-block + feed the reason back to the LLM |
| **Reversible danger** | in-workspace destruction; out-of-workspace `rm`/overwrite of git-tracked content | checkpoint → run → one-click undo |
| **Irreversible danger** | real-world side-effects; out-of-workspace destruction with no snapshot | **interactive:** pause-and-ask · **trigger:** pre-auth grant check |

The **interactive approval lane mirrors the coding-agent permission model** (the
user's explicit call: "permissions should have same design as for Claude Code").
We reuse the *design* — a `*PermissionRequested` / `*PermissionResolved` event
pair, an in-memory permission state with dedup + per-thread `session_allows`,
the `AllowScope` enum (Narrow / Broad / Session) + `derive_allow_pattern`, a
persisted allowlist file, and the existing `PermissionCard` UI — but **not** the
MCP path: the Lucidos Agent is in-process, so the block happens in the agentic
loop on the `QuestionWaitRegistry` substrate (exactly how the chat
`ask_user_question` tool already blocks and resumes), not via
`--permission-prompt-tool`.

The classifier is **hybrid**: a static fast-path settles Safe and Catastrophic
deterministically (zero latency, zero token cost); only the ambiguous middle hits
a cheap LLM-judge (Haiku) that decides reversible vs. irreversible vs. safe and
produces the card's one-line summary, **erring toward ask**.

Detailed architecture: `docs/plans/2026-06-08-agent-command-safety-design.md`.
Phased build: `docs/plans/2026-06-08-agent-command-safety-implementation.md`.

## Rationale

- **The win is selectivity, not approval.** A blanket approve-everything gate is
  exactly what the user rejected; it trains users to click through and protects
  nothing (the same "cries wolf" failure ADR 0001 identifies). Gating only the
  thin dangerous slice keeps the safe majority frictionless, which is the whole
  point.
- **Mirror, don't reinvent.** The coding-agent permission model already solved the
  hard parts — dedup of identical concurrent prompts, per-thread session-allow,
  persisted allowlist, restart-safe resolution, an answered-card render on reload.
  Mirroring its *design* (and reusing `AllowScope` / `derive_allow_pattern` /
  `PermissionCard`) gives the friction-reducers ("Allow for this thread", "Always
  allow") for free and keeps one mental model across both agents.
- **In-process, not MCP, because the Lucidos Agent is in-process.** CC needs the
  MCP permission-prompt-tool because it's a subprocess the engine can't reach
  into. The Lucidos Agent's loop runs inside the engine and already has a
  pause-and-resume primitive (`QuestionWaitRegistry`, `EventChannel::Chat`,
  `WaitingForUserAnswer`). Routing the chat gate through MCP would be a pointless
  detour.
- **Reversibility beats prevention where it's available.** Lucidos is
  event-sourced and git-backed; in-workspace destruction is cheaply recoverable
  with a checkpoint, so it routes to *undo* rather than a prompt. We only spend a
  user interruption on damage that genuinely can't be undone.
- **Triggers can't be asked.** A scheduled trigger has no human at the keyboard, so
  a runtime prompt would just deadlock it. Pre-authorizing a side-effect grant at
  trigger-creation time is the only model that lets a legitimate "email me the
  digest" trigger run autonomously while still hard-blocking ungranted side-effects.
- **Hybrid classifier balances the two failure modes.** Pure static rules are
  brittle and the LLM can phrase around them; a pure LLM-judge adds latency/cost to
  *every* command and is itself fallible. Static-for-the-obvious + judge-for-the-
  middle keeps the catastrophic cases deterministic and the common cases free,
  while the judge only runs where its judgment is actually needed.

## Consequences

- **Kept:** out-of-workspace fixing capability (location is a signal, not a wall);
  the frictionless safe path for the bulk of commands; one permission mental model
  shared with CC; restart-safe approval via the existing question substrate.
- **Added surface:** a new event pair + projection, an in-memory permission state,
  a persisted allowlist file, a classifier module, a Haiku judge call on the
  ambiguous middle, a checkpoint/undo path, and a trigger side-effect grant field.
  Each lands in its own phase so value ships before the expensive parts.
- **Given up / accepted limits:**
  - **The LLM-judge is the weakest link and is fallible.** "Is this `curl -X POST`
    a real side-effect or a harmless internal call?" is genuinely ambiguous, so the
    judge will have false positives (gating a safe command → friction) and false
    negatives (missing a novel side-effect → the screwup slips through). Mitigated
    by layering — catastrophic is caught deterministically, in-workspace misses are
    recoverable via undo, the judge errs toward ask — but there is no 100% version.
  - **Undo only covers in-workspace git-tracked content.** Out-of-workspace
    destruction is therefore routed to *ask*, not *undo*; a side-effect on the real
    world (sent email, charged card) is never undoable and only the prompt/grant
    guards it.
  - **A small latency/token cost** on the ambiguous middle (the judge call), and a
    little extra friction the first time a user hits a new dangerous category
    before they "Allow for this thread" / "Always allow" it.
- **Reopen criteria:** if the judge's false-rate proves too high in practice, fall
  back to static-only for the irreversible lane (accepting more misses) or move the
  judge inline into the main model's reasoning; if side-effects prove to flow
  mostly through dedicated tools rather than bash/python, narrow the judge's bash
  surface accordingly.

## Alternatives considered

| Option | Why rejected |
|---|---|
| **Approve every command** | Exactly what the user rejected; trains click-through, protects nothing. |
| **Static deny-list only** | Brittle; the LLM phrases around it; can't summarize for the card or judge novel side-effects. Kept *as one layer* (catastrophic + safe fast-path), not the whole answer. |
| **LLM-judge on every command** | Latency + token cost on the safe 95%; makes the common path slow for no benefit. Judge is reserved for the ambiguous middle. |
| **Hard-confine cwd to the workspace** | Kills the wanted out-of-workspace Mac-fixing feature. Blast radius is a risk signal instead. |
| **Reuse `UserQuestionAsked` for the approval card** | Works, but loses the session-allow / persisted-allowlist / dedup machinery the CC model already has — i.e. loses the friction-reducers that make "don't approve everything" real. Mirror the CC permission model instead. |
| **Reuse the MCP `--permission-prompt-tool` path for chat** | Pointless for an in-process agent; the loop already has `QuestionWaitRegistry` pause/resume. |
| **Undo-only, no prompts** | Can't cover irreversible real-world side-effects (the #1 threat) — you can't git-revert a sent email. Undo is the *reversible* lane only. |
| **Runtime prompt for triggers too** | Deadlocks an unattended trigger; nobody to click. Pre-authorize at setup instead. |

## Addendum (2026-06-25): Phase 5 grant extended to unattended coding-agent sessions

**Context.** Phase 5 gave a *trigger* a *side-effect grant* so its *Lucidos Agent*
(chat) command-guard runs could perform pre-authorized irreversible side-effects
unattended. But a trigger can also spawn a *coding-agent thread* (Claude Code /
Codex) — directly, or as an agent-spawned sub-thread of an orchestrator. Those
threads have their own permission flow (`CodingAgentPermissionRequest`, ADR 0005
for Codex), which waited **indefinitely** for a human to click the card. With no
human present, a trigger-spawned coding-agent thread that hit a sandbox
escalation / out-of-cwd write **hung forever** (observed: a nightly Codex
security scan stuck on `lucidos data write` to the workspace `data/` dir, blocked
by Codex's `workspace-write` sandbox).

**Decision.** The grant now **also governs unattended coding-agent sessions**, at
the shared permission chokepoint `engine::cc_permission::prompt_coding_agent_permission`
(both the CC MCP HTTP path and the Codex app-server bridge funnel through it):

1. **Attend mode by spawn-tree root.** `resolve_attend_mode` walks the thread's
   persisted `MessageOrigin` chain to its root. Human device at the root ⇒
   *interactive* (unchanged: emit a card, wait). Trigger/scheduler at the root ⇒
   *unattended*, inheriting that trigger's `side_effect_grant` from the in-memory
   registry (rebuilt from events at boot ⇒ restart-safe). A user-rooted tree
   stays interactive even when an agent spawned the leaf thread.
2. **Static classification, no judge.** `classify_coding_agent_request` reuses the
   command guard's `static_classify` / `fallback_classify` (commands) plus a
   workspace-containment check (file writes) — deterministic, so the permission
   path can't itself stall.
3. **Decision (`decide_unattended`).** Benign in-workspace work → allow; an
   irreversible `SideEffectCategory` in the grant → allow; an ungranted
   irreversible side-effect → deny; catastrophic → deny. The unattended path emits
   **no** card events (mirrors the session-allow fast path), so it never hangs.
4. **Deny ≠ fail-run (the difference from chat).** The chat command guard returns
   `FailTrigger` and fails the whole run on an ungranted side-effect. The
   coding-agent path denies just the **one** request — the agent receives the
   denial and routes around it or reports the step failed. Applies regardless of
   the `command_guard` toggle (the coding-agent permission flow is always on).

**Non-goals.** No change to Codex's `approvalPolicy` (kept `on-request` so an
auto-**allowed** escalation actually runs — the benign work-tracker write
succeeds). No sandbox `writable_roots` change, no new persisted events / migration,
no LLM judge in the permission path. Interactive sessions are unchanged.

Implementation: `engine/cc_permission.rs` (`AttendMode`, `resolve_attend_mode`,
`RequestVerdict`, `classify_coding_agent_request`, `decide_unattended`). Plan:
`docs/plans/2026-06-25-trigger-coding-agent-inherited-grant.md`. See also ADR 0005
(the Codex app-server approval bridge this rides on).

## Addendum (2026-07-30): in-worktree writes auto-allow; session allows are durable

**Context.** Two defects made the coding-agent permission card fire constantly on
a Lucidos-source thread, and the ADR's own thesis — *spend a user interruption
only on the dangerous slice* — is what they violated.

1. Claude Code auto-approves in-cwd writes under `acceptEdits` **except** under
   `.claude/` and `.git/`, which it routes through `--permission-prompt-tool` in
   every mode and regardless of `--allowedTools`. Lucidos keeps all of its own
   agent configuration in `.claude/`, so editing a rule or a skill cost a click
   every time — and `BROAD_ALLOW_INEFFECTIVE` already hides the persisted
   "Always allow" button for those tools precisely because CC ignores the
   pattern. In ten days of a dev workspace, **every** card raised was an
   `Edit`/`Write` under `.claude/`. That is the "cries wolf" failure ADR 0001
   names and this ADR's Rationale explicitly rejects.
2. `PermissionState::session_allows` lived only in memory, so every
   Apply-with-restart wiped it while the thread itself resumed. Observed: a
   grant at 06:16, a `ChangeApplied` at 06:18, the same file re-asking at 06:23.
   That also breaks the `CLAUDE.md` statelessness rule — a user-visible grant
   held only in a `Mutex`.

**Decision.**

1. **An in-worktree file write needs no card.** At the shared chokepoint
   `prompt_coding_agent_permission`, a request in the `coding_agent_file_target`
   vocabulary whose target resolves inside the session's *own* worktree resolves
   immediately, emitting no card events (the silent fast path session-allow and
   the unattended path already use). Justified by the worktree being disposable
   and every change in it being reviewed in the Diff before Apply. Stated as
   "in-worktree", not "`.claude/`", so Lucidos does not encode CC's undocumented
   heuristic; the only observable delta *is* CC's protected paths. Containment
   is **resolved**, not lexical — both the root and the longest existing prefix
   of the target are canonicalized, because a lexical prefix check is escapable
   through a symlink inside the worktree pointing at an external directory
   (caught in review of this very change). Four limits: a `..` component fails
   containment; a relative path fails closed (resolving one needs the agent's
   cwd, which differs between repo-rooted and app coding-agent threads; both
   backends send absolute paths); any `.git` component still asks, checked
   before *and* after resolution (git metadata is the one in-worktree location
   absent from the reviewed diff — a written hook runs on the next commit);
   commands are never covered, so a `Bash` touching `.claude/` still asks. An
   unknown or unresolvable worktree root ⇒ fail closed.
2. **`AllowScope::Session` is durable.** `hydrate_session_allows` refills a
   thread's set from the persisted `CodingAgentPermissionRequest`/`Resolved`
   pair on the first prompt after a restart, re-deriving each pattern through
   the same `derive_allow_pattern` the grant used so the two sites cannot drift.
   Lazy (one query per thread per engine lifetime, on a path that was about to
   block on a human anyway) rather than a boot sweep. Only `allowed: true` **and**
   `persist_scope: "session"` hydrate. A query error leaves the thread
   unhydrated and the card renders — it fails toward asking.

**Alternative rejected: `--permission-mode bypassPermissions`.** The user asked
about "auto mode"; `acceptEdits` already *is* CC's auto-accept-edits mode, and
the only mode that silences these cards is `bypassPermissions`. It was rejected
because CC then stops calling the permission tool at all: no card for Bash, for
out-of-workspace writes, or for external side-effects, and the Phase 5 unattended
trigger grant above goes dark with it. That trades this ADR's entire selectivity
argument for a click on rules files.

**Non-goals.** No `--permission-mode` change. No change to Bash gating, to
out-of-worktree writes, to `resolve_attend_mode` / `decide_unattended`, or to
Narrow/Broad persistence. The chat command lane
(`Engine::pending_command_permission`) shares `PermissionState` and has the same
in-memory weakness, but persists a `command` string rather than a tool `input`,
so it needs its own extractor before it can adopt hydration — deferred. No new
event type, no migration, no UI change.

Implementation: `engine/cc_permission.rs` (`worktree_write_auto_allowed`,
`path_inside_worktree`, `lookup_session_worktree`, `hydrate_session_allows`,
`WORKTREE_WRITE_ALLOW_REASON`, `PermissionState::hydrated_threads`). Plan:
`docs/plans/2026-07-30-cc-permission-worktree-writes-and-durable-session-allows.md`.

## Addendum (2026-08-06): Undo removes what the command created, and a command that captured nothing shows no card

**Context.** A user ran a `run_python` step the guard classified `ReversibleDanger`
(it contained `shutil.rmtree(...)`), clicked **Undo**, watched the card flip to
"Reverted", and nothing observable changed. Two separate holes, both hit at once:

1. The step's destruction target was `.lucidos/tmp/...`, and `.lucidos/` is
   gitignored. `create_command_checkpoint` snapshots with `git add -A`, which
   honours `.gitignore`, so the destroyed directory was **never in the snapshot**
   and Undo had nothing to restore. The card was still offered.
2. The step's output, a plugin archive under `data/artifacts/`, was created after
   the snapshot, and the Phase 4 limit above says undo never removes created
   files. So the one file the user wanted gone stayed.

Together the card offered an Undo that could neither restore what the command
destroyed nor remove what it made, and then reported success. The user's third
observation was the one that shaped the fix: **Undo was a blind button.** There
was no way to see what the step had done before clicking, or what the click did
after.

**Decision.**

1. **Bracket the command with two snapshots, not one.** The pre image is
   unchanged. After the command returns, a **post** image is written the same way
   onto `refs/lucidos/command-post-images/<id>` (a separate namespace: git cannot
   hold both `refs/x/<id>` and `refs/x/<id>/post`). One
   `git diff-tree -r -z --no-renames` over the pair names exactly what that
   command created (`A`), overwrote (`M`) and deleted (`D`).
2. **Undo removes the files the command created**, lifting the Phase 4 limit.
   This is safe where the blanket `git clean` rejected above was not, and the
   difference is the post image rather than a change of nerve: anything that
   happened *after* the command cannot be in the `A` set, so the deletion is
   scoped to that one command's own output instead of to everything untracked. A
   second guard narrows it further: a created file is removed only if git still
   calls it unchanged against the post image, so a file the user has edited
   since is kept. That comparison is git's own (`read-tree` the post tree into a
   throwaway index, an index-wide `update-index --refresh`, then `diff-files`)
   rather than a blob-sha compare, because a sha compare is wrong for a symlink:
   git stores the link target path as the blob, while hashing the path follows
   the link and reads the target file, and a dangling link does not open at all.
   The refresh deliberately takes no pathspec, since `update-index <path>`
   re-registers that path from the working tree and would make every file
   compare equal.
3. **No card when the pair shows nothing.** An empty diff means the command
   changed nothing git-visible, so Undo would be a no-op in both directions.
   Both refs are dropped and no `CommandCheckpointed` is emitted. This is the
   fix for hole 1, and it is deliberately expressed as "did the snapshot capture
   a change" rather than as a rule about which paths are ignored, so the guard
   needs no ignore-awareness of its own.
4. **The card carries a diff.** `GET /api/v1/command-checkpoint/diff` returns the
   pre-to-post diff in the same `RepoDiff` shape the repository and change diffs
   already return, so the frontend renders it with the same components. The card
   also states what Undo will do, from `restores` / `removes` counts on the
   event.
5. **`CommandCheckpointed` moves from before the command to after it.** The
   decision now depends on the outcome. The card was never actionable early in
   any case: undoing a command that is still running would fight the running
   process.
6. **The refs survive the undo** (they back the diff viewer) and are reclaimed by
   an opportunistic sweep at checkpoint-creation time, 30 days after the pre
   image's commit date. Idempotence was already guarded by the persisted
   `CommandCheckpointReverted` event rather than by the ref's absence, so keeping
   them changes nothing there.

**Accepted limits.**

- **Content the snapshot cannot capture is still unrecoverable.** Widening the
  snapshot to ignored paths is not affordable: `.lucidos/` holds coding-agent
  worktrees, which are full repo copies. What changed is that such a command now
  says nothing instead of claiming a recovery it cannot perform.
- **A concurrent writer during the command's own execution window** can land a
  file in the `A` set. The window is one command's runtime and the byte-identity
  check narrows it further; distinguishing writers would need process-level file
  tracking.
- **Undo remains working-tree-only.** Removing a file that has since been
  committed leaves an uncommitted deletion, the same shape restoring a committed
  deletion already had.

**Non-goals.** No change to the classifier, the risk lanes, the judge, the
trigger side-effect grant, or the coding-agent permission path. No per-file
selective undo: the diff is read-only and Undo stays all-or-nothing for one
command.

Implementation: `engine/git_ops/checkpoint.rs` (`create_command_post_image`,
`diff_checkpoint_effects`, `CheckpointEffects`, `remove_created_files`,
`prune_expired_checkpoints`), `engine/command_permission.rs`
(`finalize_command_checkpoint`), `api/command_checkpoint.rs`, `api/diff.rs`,
`components/chat/CheckpointDiffModal.tsx`. Plan:
`docs/plans/2026-08-06-command-checkpoint-undo-removes-created-files.md`.
