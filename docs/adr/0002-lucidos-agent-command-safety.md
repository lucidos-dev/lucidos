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
