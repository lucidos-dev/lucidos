# 0541: A refused local e2e run gives up at once with exit 75; /harden reads it as SKIP and the main check runs the specs; nothing waits for the e2e lock; supersedes 0057's subscribe-and-wait

- **Status**: Accepted
- **Date**: 2026-10-20

## Context

ADR 0057 told a run refused by the *e2e lock* to subscribe to
`E2ELockReleased` and end its turn. That fixed sleep loops, but it did not
scale. ADR 0502 made `/harden` run the touched browser specs on this host, so
every frontend change now contends for two *e2e slots*.

On 2026-10-20, 14 threads armed a wait on `E2ELockReleased` in 6 hours. One
release woke all of them. One won, the rest re-armed. Each idle thread kept
its prompt cache warm, and none of their changes could be applied meanwhile.

## Decision

A refused local e2e run gives up at once, and nothing waits for the lock:

- A run refused because every slot it needs is held exits **75**
  (`E2E_LOCK_BUSY_EXIT`, EX_TEMPFAIL).
- `/harden` reads 75 from `e2e-touched` or `e2e-touched-webkit` as `SKIP`,
  and a `SKIP` passes. The *main check* (ADR 0464) runs those specs after
  landing.
- The refusal text and the `e2e-lock-busy` skill say: rerun a committed
  selection with `--github`, or else rely on the other tests and the main
  check, and report that the spec did not run.
- The GitHub driver's local leg no longer sleeps on the lock. A busy lock is
  no verdict.

## Rationale

The touched-spec run is a fast early signal, not the gate. The main check
runs every spec after every landing, and a red one opens a fix thread. A
skipped early signal costs a later catch at worst. A parked thread costs money
for the whole wait and blocks its own Apply.

A queue in the engine would keep the signal. It is a bigger design, and it
would still park threads behind a long WebKit run.

## Consequences

- The lock, its slots and its OOM reason stay unchanged.
- `E2ELockAcquired` and `E2ELockReleased` stay. Taking the lock still stands
  down a watch armed before this ADR.
- A branch can land with a touched spec that never ran. The main check is
  the backstop for that.
- Exit 75 from any other suite stays a FAIL.

## Alternatives considered

- **Keep waiting, but cap the waiters.** Still parks threads, and a cap needs
  a fair queue to be useful.
- **Send every touched spec to GitHub.** A GitHub run pays minutes of compile
  and overhead (`docs/plans/2026-10-20-fast-test-loop.md`), and uncommitted
  work cannot go there.
- **More slots.** The slots exist because two WebKit runs OOM'd a 32 GB Mac.
