# 0478: A Switch hands an agent's processes to the session that resumes it

- **Status**: Accepted
- **Date**: 2026-10-14
- **Amends**: [0391](0391-agent-teardown-reaps-by-agent-run-marker.md), [0292](0292-harden-early-suite-run.md)

## Context

A study of 1,099 `/harden` runs found 13.5% hit a RERUN or VOID verdict.
Runs with two or more suite rounds took 58% of harden spend. Most RERUNs that
were read by hand followed an engine restart.

The path is the user's *Switch to new version*. The old engine's teardown ends
every coding-agent session and waits for each. Each driver then runs the ADR
0391 sweep, which SIGTERMs every process group carrying the session's agent
run marker. That includes the early suite run (ADR 0292), which `/harden`
starts behind a shell `&`. The new engine then auto-resumes the thread. The
agent finds its suites dead, every unfinished one reads MISSING, and the
suites run again.

ADR 0391 assumed the sweep never ran on a Switch, since the engine exits
first. It does run, because the teardown waits for the sessions to end. The
dev workspace's gateway log shows the SIGTERM line inside two Switches on one
day. One of them killed a thread's engine suite eight minutes into its run.

The plan is `docs/plans/2026-10-14-switch-hands-agent-processes-to-the-resumed-session.md`.

## Decision

A teardown that promises its threads an auto-resume reaps nothing. The
promise is `AbortCause::EngineShutdown.promises_auto_resume` on the
teardown's actor, the predicate the `paused` verdict and the resume gate use.

Each thread's agent session records its marker in
`<workspace>/.lucidos/agent-runs/<thread_id>`, beside the engine process that
wrote it. A later session of the thread on another engine process adopts that
marker. Its own end then reaps what the earlier session left running.

## Rationale

**A promised resume pauses the work; it does not end it.** The sweep exists
because an agent's processes are worthless once the agent is gone. After a
Switch the agent is back within seconds and expects them.

**Adopting the marker keeps ADR 0391's bound.** Without it the handed-over
processes would carry a marker no engine knows, and live until they finished
on their own. With it, the resumed session's turn end reaps them, exactly as
it reaps its own. A crash gains the same bound on the next Continue.

**Only a record from another engine process is adopted.** Inside one engine,
the previous session's sweep can still be running when the next session
spawns. Sharing its marker would let that sweep kill the new session. Across
engines no such sweep exists: the new engine starts only after the old one has
released the workspace's owner lock.

**The record lives in the workspace's `.lucidos/`.** It describes processes
that die at a reboot, the ephemeral class `CLAUDE.md` allows outside Postgres.
The worktree would not do: an external repo's worktree would show it as an
untracked file.

**The suite result still describes its tree.** A Switch does not touch a
worktree. The suite runner keeps its pid and start time, so `harden-suites.sh`
still recognises it. Each suite still records the paths changed at its exit,
and the verdict still voids any unsafe change since the start commit.

## Consequences

- An early suite run survives a Switch, and the resumed `/harden` joins it.
- A side question and model discovery mint their own marker and record
  nothing, so they never adopt or overwrite a thread's.
- A teardown nobody requested (`stop.sh`, an external SIGUSR1, ctrl-c) still
  reaps, as before.
- The sweep's log lines name the thread, and a handover logs the groups it
  leaves running.
- Engine-owned background tasks still die at any teardown (ADR 0257).
- The watchdog's kill and auto-resume still reaps. It is not a restart.
- One short record per coding-agent thread accumulates in `.lucidos/`.

## Alternatives considered

**`harden-suites.sh` reports the cause and resumes cheaply.** A resume mode
could keep the PASS results and re-run only the killed suites. Rejected as the
fix: the killed suite is usually the engine suite, the expensive one, and it
would still start over. It also fixes `/harden` alone, while the cause is the
engine killing work it promised to resume.

**Run the suites without the marker.** Rejected: that removes ADR 0391's bound
for the case it was written for, an abandoned suite run holding a build slot.

**Never sweep at any teardown.** Simpler, but a `stop.sh` then leaves every
agent's builds and tests running. The user asked for the work to stop.

**Derive the marker from the thread id.** Every session of a thread would
carry the same marker with no record at all. Rejected for the in-engine race
above: a turn's sweep could kill the next turn's session.

**Record the marker in Postgres.** Rejected: it would need a migration for a
value that is meaningless after a reboot.
