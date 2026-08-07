# 0005 — Codex defaults to the `codex app-server` protocol; `codex exec` stays as the escape hatch

- **Status:** Accepted
- **Date:** 2026-06-12

## Context

ADR 0004 integrated Codex as per-turn `codex exec --json` processes —
deliberately the cheapest second mover. That protocol cannot do three things
the engine's Claude Code integration treats as table stakes:

1. **Permission cards.** `codex exec` runs `approvalPolicy: never`; a command
   that needs to escalate past the sandbox fails silently instead of asking.
2. **Per-token streaming.** Items arrive whole (`item.completed`), so Codex
   threads render chunky.
3. **Graceful interrupt.** Cancel kills the per-turn child; partial work is
   lost and the turn closes with a synthesized Result.

`codex app-server` (verified against codex-cli 0.139.0; schema via
`codex app-server generate-json-schema`) is a persistent child speaking
line-delimited JSON-RPC over stdio: `thread/start` / `thread/resume`,
`turn/start` per input, `item/agentMessage/delta` streaming notifications,
`turn/interrupt`, and server→client approval requests
(`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`)
under `approvalPolicy: on-request`. It is upstream-experimental and shaped
for IDE integrations; its surface has moved between codex releases.

## Decision

1. **Two drivers behind one `CodexRuntime`, selected by
   `LUCIDOS_CODEX_PROTOCOL`.** Default **`app-server`**
   (`runtime/codex_app_server.rs`); `exec` (`runtime/codex.rs`) stays fully
   wired as the escape hatch. Both share `CodexConfig`, the sandbox profile
   (`workspace-write`, network on, and `CodexConfig.sandbox_writable_roots`
   — the shared git dir plus the workspace's `data/` tree; see ADR 0004 §4's
   2026-07-26 update), the `lucidos`
   MCP server (`ask_user_question`), and the Codex thread id — the two
   protocols read the same on-disk rollout, so an existing thread resumes
   fine after a flip.
2. **Approvals bridge onto the existing permission machinery.**
   `RunningAgent` gains `permission_rx:
   Option<mpsc::UnboundedReceiver<AgentPermissionRequest>>`; a select arm in
   `run_session/run.rs` spawns one waiter task per request (the loop never
   blocks on the user) and drives the same dedup / session-allow / emit /
   broadcast core the CC MCP HTTP path uses — extracted to
   `cc_permission::prompt_coding_agent_permission`, called by both paths.
   The user's PermissionCard click resolves the broadcast; the driver
   replies `accept` / `decline` to the JSON-RPC request. CC and the Codex
   exec driver carry `permission_rx: None`.
3. **Approval policy flips to `on-request` (app-server only).** Sandbox
   escalations raise a card instead of failing silently. The exec escape
   hatch keeps `never` + sandbox-as-guard (ADR 0004 §4).
4. **Interrupt rides the protocol.** `ControlRequest::Interrupt` →
   `turn/interrupt {threadId, turnId}`; the turn ends with
   `turn/completed {status: interrupted}` and partial work survives. The
   engine's 8s `INTERRUPT_ESCALATE_AFTER` still hard-kills via the
   cancellation token when codex ignores the request.
5. **Streaming needs no engine change.** `item/agentMessage/delta` maps onto
   `AgentEvent::Message`; the engine's buffer/flush loop already tolerates
   arbitrary chunk sizes. The `item/completed` full-text echo emits only the
   un-streamed remainder so text never duplicates.

## Rationale

The app-server protocol is the only Codex surface that can host the
permission flow — and the permission flow is what makes `on-request`
approval safe to enable, which in turn is what lets a Codex session do
things the sandbox would otherwise silently block. Streaming and graceful
interrupt come along for free on the same protocol. Keeping exec wired (not
deleted) bounds the risk of betting on an experimental upstream surface: one
env var rolls a workspace back to the shipped-and-stable model.

## Consequences

- Codex threads get PermissionCards (`tool_name: command_execution` /
  `file_change`), per-token streaming, and cancel-preserves-partial-work.
- A codex upgrade that breaks the app-server contract is mitigated three
  ways: the negotiated `initialize` response is logged, the protocol mapper
  is pinned by stub tests, and `LUCIDOS_CODEX_PROTOCOL=exec` restores the
  per-turn model without a rebuild.
- The engine still terminates the agent subprocess at idle
  (`IdleAction::ExitSubprocess`) — "persistent" means within a turn-set, not
  across idle gaps; resume goes through `thread/resume`.

## Alternatives considered

- **Stay on exec and accept the gaps** — rejected: silent sandbox failures
  are the worst of the three gaps; users see a Codex turn "finish" without
  doing what it was asked, with no signal that an approval was needed.
- **Delete the exec driver once app-server lands** — rejected: app-server is
  explicitly experimental upstream; betting the only Codex path on it
  removes the rollback when (not if) its shape moves.
- **Drive approvals through the MCP permission server (like CC)** —
  rejected: codex has no `--permission-prompt-tool` designation; its
  approvals are first-class JSON-RPC requests on the app-server protocol,
  so the bridge is the natural seam and reuses the engine machinery anyway.

## Addendum (2026-06-18): mid-turn follow-ups interrupt-and-redirect

**Context.** The Codex drivers accept input only at a turn boundary
(`turn/start` requires no turn in flight; the exec driver is per-turn), so the
app-server driver queues any `AgentInput` that arrives mid-turn ("No mid-turn
injection … every accepted input gets its own turn"). A user follow-up sent
during a long autonomous turn therefore sat invisibly in the driver's queue
until the turn finished — observed as "Codex isn't getting my follow-ups at all"
on a turn that ran 18+ minutes without idling. Claude Code differs: its driver
writes follow-ups to the CC process's stdin immediately, so the live turn is
steered.

**Decision.** A genuine **user** follow-up that lands while a **Codex** turn is
in flight now **interrupts the live turn and runs the follow-up as the next
turn** on the same Codex thread. The interrupt is the graceful `turn/interrupt`
from §4, so partial work survives and the thread keeps full context; the
interrupted turn ends as a resumable `Canceled` boundary (no spurious
`ResponseGenerated`, no change proposed for the redirected-away work). This is
**Codex-only** — CC keeps its stdin steering — and **mid-turn-only** (a follow-up
to an idle-but-alive Codex session still routes via `turn/start` immediately).

**Mechanism (no driver change).** The seam is the engine's follow-up fast-path
(`engine/chat/process/run.rs`), which reuses the existing Stop-button machinery
(`interrupt_agent`'s `cancel_actor` + `interrupt` notify) and the existing
"interrupt superseded by inflight follow-ups" accounting
(`terminate_decision::KeepAliveForFollowup`): the fast-path **reserves** the
subprocess (`AgentSession::redirect_followup_pending`) so the interrupted turn's
idle keeps it alive, fires the interrupt, waits for idle (so the `Canceled`
terminal is sequenced before the follow-up's `MessageReceived`), then routes the
follow-up normally. The reservation is a flag rather than a count because it has to
hold across a window in which the message provably is not in `msg_rx` yet, and the
idle decision takes it, so it is worth exactly one idle. (Until 2026-08-07 this was
a pre-count on a `pending_followups` counter; see
`docs/plans/2026-08-07-api-drop-resume-suppressed-by-phantom-followup-count.md`.)
The app-server driver is unchanged: it still queues the input and
runs it the instant the interrupted turn ends. Gated on
`AgentSession::coding_agent == Codex` and `is_in_flight()`; never fires for an
engine-internal child-wake. Decision + arming are pure functions
(`should_redirect_followup` / `arm_followup_redirect` in
`engine/chat/process_helpers.rs`) with unit tests. (Named
`should_redirect_codex_followup` / `arm_codex_redirect` until 2026-08-06, when
Claude Code gained the same interrupt behind an opt-in `urgent` flag and the
mechanism stopped being Codex-only. Codex still redirects unconditionally, and
that is what `urgent` being a no-op on Codex means: its protocols cannot surface
a queued message mid-turn at all, so there is no gentler mode to opt out of. See
`docs/plans/2026-08-06-urgent-child-follow-up-preempts-in-flight-work.md`.)

**Labeling (2026-06-21 follow-up).** The interrupted turn now carries a dedicated
`CancelCause::SupersededByFollowup` instead of `UserStop`. The user steered, they
didn't Stop, so the frontend renders it **neutrally** — the interrupted turn reads
a plain "Done", with no "Canceled ✕" badge and no standalone "Response canceled"
panel, exactly like a chat/CC follow-up. The cancel is still emitted (it's what
suppresses a spurious `ResponseGenerated` / change proposal); only the cause and
its rendering changed. `arm_followup_redirect` flags the session; the run_session
interrupt arm drains the flag into `classify_result` (and the escalation
fallback). `CancelCause` is not part of the generated TS contract, so this needed
only the hand-maintained frontend `CancelCause` union — no `ThreadEvent` variant
change. Both other lanes now reach the same cause the same way: Claude Code
through the shared `arm_followup_redirect`, and the Lucidos Agent through
`cancel_thread_for_followup` + `cancel_cause_for_turn`. See `docs/plans/2026-06-21-codex-followup-redirect-label.md`.

## Addendum (2026-06-25): unattended trigger sessions auto-resolve approvals

The §2 approval bridge raises `CodingAgentPermissionRequest` and waits for a human
to click the card. A Codex session spawned by a **trigger** has no human — so it
hung forever on the first sandbox escalation. The approval flow now auto-resolves
for unattended (trigger-rooted) sessions at the shared engine chokepoint
(`engine::cc_permission::prompt_coding_agent_permission`): benign in-workspace work
is allowed, an irreversible side-effect is allowed iff the originating trigger's
*side-effect grant* covers it, and ungranted / catastrophic requests are denied —
no card, never hangs. `approvalPolicy` stays `on-request` (an auto-**allowed**
escalation must still `accept` so Codex re-runs the command escalated). Full
decision: ADR 0002 (Phase 5 addendum, 2026-06-25).
