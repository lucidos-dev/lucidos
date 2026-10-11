# 0391: An agent's teardown reaps every process group carrying its agent run marker, since Claude Code puts each Bash call in a group of its own

- **Status**: Accepted
- **Date**: 2026-10-08

## Context

An Apply build waited on 6 of 18 cores while one of the 3 build slots was
held by a `cargo test` run. Its test binary had spun for 2h20m, re-parented to
launchd, after the session that started it was gone. A second orphan of the
same shape, a `harden-suites.sh start` run, held another slot at the same time.

The engine spawns each coding agent as the leader of its own process group,
and its teardown signals that group. Claude Code runs every Bash tool call in
a NEW process group: the `claude` pid leads one group, its `zsh -c` child
leads another. So the teardown never reached a test, build or server an agent
started. The engine prompt, ADR 0257 and `harden-suites.sh` all claimed it
did. The teardown also ran only while the agent child was unreaped, so an
agent that exited on its own triggered no kill at all.

Nothing bounded an engine test run either. `lucidos build-slot` waits on its
child with no limit, and `--max-wait` bounds only the wait for a slot.

## Decision

Every agent spawn carries a fresh random **agent run marker** in its
environment as `LUCIDOS_AGENT_RUN`, stamped by `apply_lucidos_env`. When an
agent's driver ends, the engine finds every same-uid process carrying that
marker and tears down its process group: SIGTERM, a grace, a fresh scan, then
SIGKILL for what the scan still finds.

`test-engine.sh` runs cargo under `scripts/with-run-limit.sh`, inside the build
slot. A run past `LUCIDOS_TEST_RUN_LIMIT_SECS` (default 45 minutes) has its pid
tree stopped and exits 124.

## Rationale

**The environment is the fact that survives re-parenting.** ADR 0251 settled
this for e2e browsers. A re-parented orphan identifies nobody by ancestry, and
its group says nothing once its leader is gone. Every process the agent starts
inherits the marker, whatever group it lands in, and no prompt can write it
into another process (ADR 0025).

**Fresh per spawn, so only this agent's descendants can carry it.** The
engine, the gateway and every other session started before the marker
existed. Engine-owned background tasks are spawned by the engine, not the
agent, so they never inherit it and keep their own watchdog (ADR 0257).

**A daemon an agent launches is exec'd without it.** An agent can run
`web-dev.sh` from the real checkout. Marked, the gateway would pass the marker
to every engine it spawns, and the sweep would kill them all. So
`scripts/lib/workspace.sh` unsets it for the gateway and the shared
build-watch. Unsetting it inside the process would not help: both macOS and
Linux report the environment a process was exec'd with.

**Groups, not processes.** The sweep signals each group that holds a marked
process. A group usually mixes readable processes (`cargo`, `rustc`, test
binaries, `node`, the `lucidos` CLI) with Apple platform binaries (`zsh`,
`bash`, `sleep`), whose environment macOS hides. Signalling the group reaches
both.

**The re-scan makes SIGKILL safe.** A group id from the first scan may have
been recycled during the grace. Only a group still holding a marked process
after the grace is killed.

**The run limit lives in the test script, not in `build-slot`.** The build
shares the caller's process group on purpose (ADR 0070), so the wrapper cannot
signal the build's group without also signalling its caller. A pid-tree walk
from the wrapper's own child can.

## Consequences

- A test, build or server an agent starts from Bash now dies when the agent's
  driver ends, as the engine prompt always said.
- **The shared sccache daemon is exempt.** The agents' daemon (ADR 0343) is
  forked on demand by whichever agent builds first, so it inherits that
  agent's marker. sccache sets `SCCACHE_START_SERVER=1` in the server's
  environment, and the sweep skips any process carrying it. Another shared
  daemon an agent forks on demand needs the same treatment.
- **A group of only platform binaries escapes.** A bare `sleep 600 &` or a
  shell loop has no readable environment on macOS. Such a group holds no build
  slot and little memory, so it lives until a reboot. Linux reads every
  environment.
- **Codex sweeps when its session ends.** Its driver bakes one environment
  for the session. A session ends at every idle with no follow-up queued
  (`lifecycle::terminate_decision`), so in practice that is each turn end, as
  for Claude Code.
- Processes started before this change carry no marker. They are not reaped.
- **The sweep needs the engine alive when the driver ends.** An engine that
  exits first, on a crash or a Switch, never runs it. The new engine does not
  know the old markers. An engine test run still ends at its run limit;
  anything else lives until a reboot.
- A hung engine test run frees its slot within the run limit. The libtest
  "has been running for over 60 seconds" lines name the test.

## Alternatives considered

**Watch the session from inside `build-slot`.** Make the wrapper exit when the
agent's pid dies, through kqueue `NOTE_EXIT`. Rejected: it covers builds only,
not a dev server or an e2e run. It also needs a session pid the engine does
not hand to Bash calls.

**Kill by ancestry or by cwd.** Rejected for ADR 0251's reason: an orphan is
re-parented to launchd, and a worktree's cwd is shared by every session that
ever ran there.

**Put each Bash call back in the agent's group.** Not ours to change: Claude
Code chooses the group, and Codex has its own process model.

**Exempt daemons by name.** Rejected: a name is argv, which any process can
set. `SCCACHE_START_SERVER=1` is an environment entry sccache itself writes.
