# 0025 — The host-process kill guard is not defeatable by its own caller

- **Status** — Accepted
- **Date** — 2026-07-28

## Context

On 2026-07-28 a Claude Code session killed the machine's live dev engine **twice**
— 16:25:14 and 20:10:33 — by running `scripts/lib/ports_test.sh`, the unit-test
file for the port allocator. Both times the engine logged
`[Shutdown] Shutting down gracefully...`, tore down the coding-agent threads it
owned, and exited; the gateway noticed
(`respawning 'dev' after 6 missed probe(s)`) and started a fresh one.

The chain ran entirely inside `scripts/lib/ports.sh`:

1. `test_collision_walks_forward` sets `OCCUPIED_PORTS="5173"`. The suite stubbed
   `port_is_free` and `docker` — but not `lsof`.
2. `_port_is_ours_or_free` therefore ran the real `lsof -ti :5173 -sTCP:LISTEN`
   and got back the pid of the user's live engine.
3. It handed that pid to `_try_reclaim_stale_lucidos_on_port`, whose job is to
   clear a *stale orphan* off a port the workspace wants.
4. The safety gate, `is_protected_host_pid`, **failed open**. It had two arms and
   the test defeated both — not maliciously, but by being a well-behaved
   sandboxed test: `reset_env()` does `unset LUCIDOS_HOST_PID LUCIDOS_FRONTEND_PID`,
   and the pidfile arm globbed `"$HOME"/workspaces/*/.lucidos/engine.pid` while the
   test had `export HOME="$(mktemp -d)"`. The real
   `~/workspaces/dev/.lucidos/engine.pid` was invisible.
5. `ps -p <pid> -o command=` matched `*lucidos-engine*`, so the helper declared it
   a stale orphan and sent `kill -USR1` — the engine's *legitimate* graceful-stop
   signal (it deliberately ignores SIGTERM). The engine obeyed.

Two details make this worse than "a test had a missing stub":

- The cmdline check reads like a safety net and is not one. A live host engine
  matches `*lucidos-engine*` **by definition** — it is the same binary as the
  stale orphan the helper is hunting. Everything downstream of
  `is_protected_host_pid` signals.
- The 3-second "did it release the port?" poll calls `port_is_free`, which the
  test had stubbed to keep answering "occupied". So the path always spent its
  full budget and escalated to SIGTERM and then **SIGKILL**, gated only by a
  `kill -0` re-check. The engine happened to exit inside the window both times.
  A slower shutdown would have been SIGKILLed mid-drain.

The generalisable defect is not the missing stub. It is that **every arm of a
guard whose sole job is protecting the machine-global engine read state the
caller owns** — an environment variable and a `$HOME`-relative path. Any caller
could switch the guard off, and the most ordinary reason to do so (isolating a
test) was enough.

## Decision

**A guard that protects host processes must rest on facts the caller cannot
rewrite.** `is_protected_host_pid` gains two such arms, additively — it may gain
reasons to refuse, never reasons to permit:

1. **Ancestor arm (load-bearing).** Refuse any pid that is this process or an
   ancestor of it, from a cached `ps -o ppid=` walk starting at `$$`. A process
   cannot unset its own parentage. On the machine where the incident happened
   the set is `zsh → claude → lucidos-engine → lucidos-gateway → bash`, so it
   contains both the engine and the gateway.
2. **`pid <= 1`** is protected outright, mirroring `webkit_reaper.sh::reap_once`.

The existing pidfile arm additionally scans the home recorded in the **password
database** when it differs from `$HOME` (resolved via `dscl` / `getent`, never
`eval`). The ancestor walk only reaches processes this one descends from, so a
*sibling* workspace's engine still depends on the pidfile scan — and reassigning
`HOME` must not disarm that either.

The env-var arm stays exactly as it was. It is the cheapest and most precise
signal when it is present; it is simply no longer the only thing standing
between a port collision and a dead engine.

Second, independently: **a test file must be structurally incapable of signalling
a process it did not spawn.** `ports_test.sh` stubs `lsof` for the whole file off
the same state as `port_is_free`, and installs a `kill` shim that passes `kill -0`
through, refuses every lethal signal to a pid outside its own set, records the
refusal, and fails the suite at the end of the run if any was recorded.

### Rejected: refusing based on port provenance

The considered alternative was to have `_try_reclaim_stale_lucidos_on_port`
refuse whenever the candidate holds a port the caller did not allocate. Rejected:

- **There is no sound notion of "caller-allocated" at that point.**
  `allocate_ports` is in the middle of *deciding* which port to take. A
  walk-forward candidate is by definition not yet in the registry, and the
  pinned-port branch exists precisely to reclaim a port the workspace does not
  currently hold.
- **It would re-open the drift regression.** A workspace's own crashed engine
  squatting its registered port used to make `allocate_ports` walk forward and
  persist the drift on every restart — the bug
  `test_stale_lucidos_engine_reclaimed_no_drift` exists to prevent. A provenance
  rule re-creates it.
- **It trades a hard fact for soft state.** Process parentage is maintained by
  the kernel. Port provenance is bookkeeping the same caller writes. Stacking a
  weaker check on a stronger one adds failure modes, not safety.

## Consequences

- **Protection is deliberately not sandboxable.** A test that reassigns `HOME`
  still gets an isolated port *registry* (`LUCIDOS_PORT_REGISTRY` remains
  `$HOME`-relative), but it no longer gets an unprotected engine. A test that
  needs a pid to be unprotected must use one that is genuinely dead — the
  `kill -0` liveness gate on the pidfile arms still applies — or synthetic.
- **The reclaim path can now refuse where it previously acted.** If a workspace's
  engine is genuinely an ancestor of the process allocating ports, that port is
  not reclaimable. This is correct: an ancestor engine is running, not stale. The
  `--engine-only` Apply restart is unaffected — `allocate_ports` short-circuits
  under `ENGINE_ONLY` before any reclaim, and `kill_stale_processes` /
  `wait_for_engine_shutdown` signal pidfile-derived pids directly without
  consulting the guard.
- **Cost is one `ps` fork per ancestor level, once per process**, then cached.
  The cache is assigned to a global directly rather than returned through
  `$(...)`, which would run the helper in a subshell and discard it every call.
- **`webkit_reaper.sh` only gets more conservative** — its
  `is_protected_host_pid` call is a `continue` guard, so extra refusals can only
  skip kills.
- **A missing stub is now a red test run, not a dead engine.** The
  `ports_test.sh` kill shim blocks the signal *and* records it; the end-of-run
  assertion turns any recorded refusal into a suite failure. This fired for real
  while implementing the fix: the pre-fix regression test drove the reclaim path
  into `kill -USR1 <parent shell>` and the shim refused it.
- **Not a temporary measure.** There is no condition under which this guard
  should be removed, so it gets no row in `docs/temporary-measures.md`.

## See also

- `scripts/lib/ports.sh` — `is_protected_host_pid`, `_ensure_ancestor_pid_set`,
  `_try_reclaim_stale_lucidos_on_port`
- `scripts/lib/ports_test.sh` — the suite-level `lsof` stub and `kill` shim
- `.claude/hooks/pre-kill.sh` — the sibling guard for `kill` / `pkill` /
  `lsof | xargs kill` typed directly into a Bash tool call. It inspects the
  command string, so it cannot see a kill several frames deep inside a sourced
  shell library; that is the gap this ADR closes.
- ADR 0002 — Lucidos Agent command safety (gate the dangerous slice)
- ADR 0021 — the long-lived dev stack never runs from a coding-agent worktree
- `docs/plans/2026-07-28-unspoofable-host-pid-kill-guard.md`
