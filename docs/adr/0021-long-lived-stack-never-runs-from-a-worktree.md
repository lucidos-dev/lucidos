# 0021 — The long-lived dev stack never runs from a coding-agent worktree

- **Status** — Accepted
- **Date** — 2026-07-26

## Context

A user reported a CSS change that "made no difference" after Apply. The change was
correct and merged; the frontend bundle the engine served was hours old. Diagnosis
found the **entire running stack** executing out of a coding-agent worktree:

```
gateway binary        <ws>/.lucidos/worktrees/thread-e481bacb/target/debug/lucidos-gateway
LUCIDOS_ENGINE_BIN    <ws>/.lucidos/worktrees/thread-e481bacb/target/debug/lucidos-engine
LUCIDOS_STATIC_DIR    <ws>/.lucidos/worktrees/thread-e481bacb/crates/lucidos-app/dist
gateway PWD           <ws>/.lucidos/worktrees/thread-e481bacb
```

`git worktree list` reported only the main checkout — that worktree had already been
pruned from git's registry. The live stack depended on an **orphaned directory**,
frozen at a commit on a dead `claude-code/*` branch.

Why it happened, and why it stuck:

1. A *worktree is a complete copy of the repo*, `scripts/` included. `web-dev.sh`
   resolves `PROJECT_DIR="$(dirname "$SCRIPT_DIR")"`, so invoking it from a worktree
   silently resolves the whole stack there.
2. `gateway::stack::spawn_engine` **inherits the gateway's environment**, so a
   worktree-rooted `LUCIDOS_STATIC_DIR` / `LUCIDOS_ENGINE_BIN` propagated into every
   engine it spawned — the pin survived engine restarts.
3. `gateway::server::reload_gateway` **re-execs onto the on-disk binary**, so a
   worktree-built gateway re-adopted itself — the pin survived gateway restarts too.

The user-visible symptom was silent. The checkout-level `vite build --watch` correctly
republished the *real* checkout's `dist/`; the engine served the worktree's frozen copy
and could never see it. `frontend_refresh`'s rebuild wait timed out every time and
returned with a comment calling it a "safe no-op". So every frontend-only Apply for
hours looked exactly like "my change did nothing".

## Decision

**A long-lived stack (gateway + engine + served `dist/`) must never be rooted in
`.lucidos/worktrees/**`.** Enforced at every point where the pin could form:

| Site | Behaviour |
|---|---|
| `scripts/web-dev.sh`, `scripts/tauri-dev.sh`, and the `LUCIDOS_STATIC_DIR` export sites in `scripts/lib/workspace.sh` | Refuse to start; name the real checkout and the command to run |
| The **gateway** launch path specifically (`start_gateway`, and `web-dev.sh` unless `LUCIDOS_NO_GATEWAY=1`) | Refuse **unconditionally** — the opt-out does not apply |
| `web-dev.sh --engine-build` | **Exempt** — compiles and exits before `swap_ports`/`start_gateway`, so it starts nothing and exports no `LUCIDOS_STATIC_DIR` |
| `gateway::server::validate_engine_bin` | Refuse at boot when `LUCIDOS_ENGINE_BIN` is in a worktree |
| `gateway::server::reload_gateway` | Refuse the re-exec; stay on the current image |
| `gateway::stack::spawn_engine` | Drop a worktree-rooted `LUCIDOS_STATIC_DIR` rather than pass it on |
| `engine::frontend_refresh` | Warn once when the served `dist/` is worktree-pinned; emit `FrontendUpdateStranded` when an Apply actually strands |

`LUCIDOS_ALLOW_WORKTREE_STACK=1` is the single, explicit opt-out — but it is honoured
only in **`stack` scope** (a session-scoped direct engine). In **`gateway` scope** it is
ignored outright. `scripts/lib/e2e.sh` sets it and only ever reaches `stack` scope,
because the e2e harness calls `start_engine` directly and never starts a gateway.

## Rationale

**Fail loudly rather than auto-correct the path.** Re-pointing a worktree invocation at
"the real checkout" requires guessing which clone is canonical — a machine can have
several — and silently running different code than the operator invoked is the same
class of surprise as the bug. The refusal reads the worktree's `.git` file (a linked
worktree's is a file containing `gitdir: <main>/.git/worktrees/<name>`) to name the
exact `cd` target, so failing loudly is still actionable.

**The gateway cannot fail loudly, so it degrades deliberately.** No operator is watching
the adoption or spawn paths. There, refusing the action and keeping the working image —
and dropping the poisoned variable rather than forwarding it — is the safe move. An
engine serving *nothing* is a visible failure; an engine serving something frozen is an
invisible one, and `LUCIDOS_STATIC_DIR` is already optional for headless engines.

**An explicit env opt-out, not a workspace-name check.** e2e legitimately runs from a
worktree — that is the whole point of a coding-agent session testing its own checkout.
Keying the exception on the `e2e-test` workspace name would fail *open* if that name ever
changed, and would be invisible at the call site.

**But the opt-out stops at the gateway, and that asymmetry is the core of this ADR.**
The hazard is not "a worktree" — it is a **machine-global daemon** rooted in one.
`run_gateway_supervised` traps `SIGHUP/SIGINT/SIGTERM` and is `disown`ed *specifically so
the gateway outlives the launching shell*, and a `-b` run stops the existing gateway and
relaunches it from whatever checkout invoked it. So
`./scripts/web-dev.sh -w e2e-test -b` from a coding-agent worktree — which the CC
instructions actively recommended for running e2e — kills the user's gateway and replaces
it with one pinned to a throwaway checkout, which then adopts every workspace and serves
them all its frozen `dist/`. That is the incident, reachable from a documented workflow,
so no opt-in may buy it. A session that wants to exercise its own checkout runs the e2e scripts, which boot a
session-scoped direct engine themselves and never touch the gateway.

**Predicate matches the adjacent `.lucidos` + `worktrees` component pair**, and does no
filesystem access. A substring test would catch `~/worktrees/lucidos` (a legitimate
checkout location) and `.lucidos/served-frontend`; and requiring the path to exist would
fail open for an *orphaned* worktree — exactly the case that caused the incident.

## Consequences

- Restarting a workspace from inside a coding-agent session now fails with an actionable
  message instead of silently pinning the stack. This is a deliberate papercut: the
  alternative was hours of invisible data loss on every frontend change.
- e2e is unaffected, but the opt-in is now load-bearing — removing
  `LUCIDOS_ALLOW_WORKTREE_STACK=1` from `scripts/lib/e2e.sh` breaks e2e's frontend
  serving rather than merely warning.
- **The build-only mode stays available from a worktree.** `--engine-build` (the
  Apply-triggered background rebuild) only compiles, so guarding it would break a workflow
  that cannot pin anything. The guard therefore runs *after* argument parsing — the mode
  determines the scope.
- **`./scripts/web-dev.sh -w e2e-test -b` from a coding-agent worktree is now refused**,
  and the CC session instructions that recommended it have been rewritten
  (`agent_session/prompts.rs`, `CLAUDE.md`, the `run-e2e` skill). The replacement is not a
  differently-flagged `web-dev.sh` — it is **no `web-dev.sh` at all**: `./scripts/e2e.sh`
  (and `e2e-api.sh` / `e2e-browser.sh`) already build the engine + SDK and boot a
  session-scoped engine via `ensure_workspace_running`, so the pre-start step was always
  redundant. (`LUCIDOS_NO_GATEWAY=1` alone does NOT unblock `web-dev.sh` — it drops to
  `stack` scope, which still requires the opt-in that `web-dev.sh` does not set.)
- A stack already pinned before this change is **not** auto-repaired; detection and
  refusal only apply on the next launch. Recovery is the operator's, from the real
  checkout: `./scripts/stop.sh -w <ws>` then `./scripts/web-dev.sh -w <ws> -b`.
- Orphaned worktree directories are still not garbage-collected. Related, but deleting a
  directory a process is currently running from needs its own safety analysis.

## See also

- ADR 0014 — dev runtime topology (gateway + engine, `LUCIDOS_STATIC_DIR` serving path)
- `docs/plans/2026-07-26-worktree-pinned-stack-guard.md` — the implementation plan
- `.claude/rules/dev-runtime.md` — the day-to-day rule for launching a workspace
