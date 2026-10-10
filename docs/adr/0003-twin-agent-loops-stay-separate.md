# 0003 — Agent-session loop and chat agentic loop stay separate; no shared loop orchestrator

- **Status:** Accepted
- **Date:** 2026-06-12

## Context

An architecture review flagged the three big turn-driving files —
`engine/agent_session/run_session/run.rs` (~1.7k lines, the *Claude Code
session* loop), `engine/agentic_loop/run.rs` (~1.5k lines, the *Lucidos
Agent* chat tool loop), and `engine/chat/process/run.rs` (~1k lines, message
routing) — as duplicated "turn machinery" (stream flushing, terminal
classification, idle settling, watchdog) and proposed deepening a single
**turn-lifecycle seam** both loops would run on, estimating ~60% of the
machinery was re-implemented per loop.

Implementation reconnaissance found the premise mostly stale:

- `engine/agent_session/lifecycle.rs` already **is** that seam for the
  agent-session side. Every termination decision is a pure, individually
  documented, individually tested function — `classify_result` (precedence
  explicitly documented and pinned by 25 tests in
  `lifecycle_tests/classify.rs`), `idle_action`, `terminate_decision`,
  `may_touch_change_state_at_idle`, `terminal_clears_user_hit_stop`,
  `should_auto_commit_on_cleanup`, `stop_terminal_kind`,
  `classify_session_end_action`, `safety_net_action`, `watchdog_gate`.
- The chat agentic loop does **not** duplicate these: it has no subprocess
  `Result` to classify. Its terminal emission goes through the typed
  `thread_events::emit_response_canceled` / `emit_response_aborted` helpers
  (`.claude/rules/rust.md` § "Response termination uses the typed helpers"),
  which is the shared seam for the cancel-vs-abort split across BOTH loops.
- What remains in the big files is concretely different glue: CC wire-protocol
  handling, worktree/change lifecycle, and `--resume` mechanics on one side;
  LLM tool dispatch, the command guard, and context trimming on the other.

## Decision

**Keep the two loops separate.** Shared *decisions* continue to live in
`agent_session/lifecycle.rs` (CC-side) and the typed terminal helpers in
`thread_events` (both sides). Do not build a shared "turn lifecycle
orchestrator" / "loop framework" that both loops instantiate.

## Rationale

A shared orchestrator fails the depth test that motivated it. The two loops
agree on vocabulary (turn, terminal, idle) but on almost no concrete
behavior: their inputs (subprocess event stream vs. LLM streaming + tool
calls), their wait states (CC waiting/resume vs. in-process question/
permission blocks), and their cleanup (worktree + change proposal vs.
response persistence) differ everywhere it matters. An orchestrator generic
over both would push every difference into configuration, callbacks, or
generics — an interface as complex as the two implementations combined,
which is the definition of a shallow module. The genuinely shareable parts
were already extracted by prior work, function by function, which is the
correct grain: each helper has one honest signature and its own tests.

## Consequences

- **Kept:** two long but linear loop files whose decision logic is
  delegated to small pure functions; one place per decision; tests at the
  decision grain.
- **Given up:** a future second *coding agent* (e.g. `CodingAgent::Codex`)
  reuses `lifecycle.rs` only to the extent its semantics match CC's — any
  divergence gets its own helper rather than a framework knob.
- File-length pain in `run_session/run.rs` is real but is a *navigability*
  concern: address it (if at all) by splitting the file into section modules
  along its existing seams (init/spawn, event arms, finalize), not by
  abstracting across the two loops.

## Alternatives considered

| Option | Why rejected |
|---|---|
| **Shared loop orchestrator** (one generic turn state machine, two adapters) | Interface would carry every behavioral difference as config/callbacks; deletes no complexity, relocates it behind generics. Shallow by construction. |
| **Merge chat termination into `lifecycle.rs`** | Chat has no CC `Result`; the only shared part (cancel-vs-abort typing) already lives in the `thread_events` typed helpers both loops use. Moving it would split that seam, not deepen it. |
| **Status quo with no record** | The "unify the twin loops" idea is attractive enough that reviews will keep re-proposing it — this ADR exists to carry the reconnaissance result forward. |
