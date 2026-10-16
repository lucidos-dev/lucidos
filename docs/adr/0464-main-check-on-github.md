# 0464: The main check: after a landing moves main, the engine runs the engine suite and every e2e suite on GitHub against a stripped copy of main, one at a time; it gates nothing, and a red one opens one fix thread

- **Status**: Accepted
- **Date**: 2026-10-13
- **Amends**: [0382](0382-e2e-runs-on-github-from-a-stripped-temp-branch.md),
  [0386](0386-e2e-defaults-to-github.md)

## Context

The landing lane (ADR 0463) lands clean changes at once and gates a clean
batch with nothing. Each change passed its own `/harden`, but no one tests the
combination that lands. Our branch suites fail about 15% of the time, so a
merge-queue gate on every landing would spend its gain on bisection. The plan,
`docs/plans/2026-10-12-landing-lane.md` (decisions 5 and 6), settled on a check
after the fact instead.

ADR 0382 and ADR 0386 already run e2e on GitHub from a stripped, scanned,
parentless commit. Their limits said nothing triggers per push, and `/harden`
stays the gate. The mirror push gate (ADR 0462) shipped first, because this
check adds an unattended pusher.

## Decision

**After a landing moves `main`, the engine runs the main check:** the engine
suite plus every e2e suite, on GitHub, against a stripped copy of `main`.

- **What starts one.** A `ChangeApplied` carrying a `landing_merge_sha`. Only
  the landing lane writes one, so only a Lucidos-source landing counts. The
  coordinator hears it on the EventBus, so the lane never names the check.
- **What it tests.** `scripts/main-check.sh <sha>` runs
  `e2e-github.sh main-check --commit <sha>`. The driver strips that commit,
  never a worktree's HEAD and never uncommitted files.
- **One at a time.** A landing while a check runs marks the next one due. When
  the check ends, one check covers everything that landed meanwhile, from the
  last check with a verdict.
- **It gates nothing.** No landing waits on it, and a GitHub outage blocks no
  landing.
- **No verdict is its own answer.** GitHub down, a refused push, or a restart
  mid-run is `MainCheckAbandoned`. It starts nothing by itself: the next
  landing does. There are no timers and no retries. The one bound is the
  wait on the GitHub run (`E2E_GITHUB_RUN_WAIT_SECS`). A run that never
  reports ends in no verdict, so no check stays running forever.
- **A red check opens one fix thread.** It is an ordinary coding-agent thread
  in the Lucidos repo, opened the way the lane opens its lane thread. Its first
  prompt carries the failure, its tail, the run, the range, and the changes
  that landed in it.
- **One red at a time.** While red is open, no check starts. A landing from the
  fix thread closes the red and starts the next check. So does the
  maintainer's Close, when something landed meanwhile.
- **Only this checkout runs it.** It needs `scripts/main-check.sh`, the
  private release library and the `lucidos` remote. Without them the check
  stays idle and its snapshot says why.

The events are `MainCheckStarted`, `MainCheckPassed`, `MainCheckFailed`,
`MainCheckAbandoned` and `MainCheckRedClosed`, on aggregate `main_check`
(`.claude/rules/db.md`). The snapshot rides `GET /api/v1/changes`, every
`ChangesUpdated` and `GET /api/v1/main-check`. `lucidos main-check status`
prints it, and `/harden` Phase 4 reads it before fixing a failure that also
fails on `main`.

### Amendments

- **ADR 0382 and ADR 0386.** "Nothing triggers per push to `main` or per PR"
  still holds. The main check is one more caller that asks for a run, and the
  engine is that caller. It runs after the fact, never per push, and never
  gates. Nothing deploys, and no workflow holds a secret.
- **The workflow gains an engine job.** It runs `test-engine.sh` against Docker
  Postgres on Linux, only when the request asks for the engine suite. It saves
  no cache. `scripts/check-e2e-workflow.sh` now also holds every job but
  `seed-cache` behind the plan job, and every action to a commit pin.

## Rationale

- **After the fact is the affordable test of the combination.** Clean work
  keeps landing in seconds. The check reports within one run of the landing,
  and its red rate will say whether a general two-step gate is worth a plan.
- **One check, one red, no timers.** Coalescing keeps GitHub to one run at a
  time, and a red that blocks checks keeps one failure from paging twice. A
  check that waits on a clock is a retry with no end condition.
- **The fix thread owns the red.** A branch `/harden` that meets the same
  failure leaves it alone, so ten branches do not fix one bug ten ways.
- **The engine owns the dispatch.** Events make the check restartable: a
  restart folds them back, abandons the check it cut off, and re-prompts a fix
  thread that never got its prompt.

## Consequences

- Every landing costs one GitHub run at most, coalesced. Recent mirror e2e runs
  took 11 to 40 minutes, so the engine job runs beside the shards. The e2e
  shards stay whole for now (plan Open Question 2).
- The four specs that need a real Claude Code do not run in the main check.
  The local leg could test only this checkout's files, never the snapshot.
- `--commit` works for every e2e selection, and drops the local leg the same
  way. Its browser specs are listed and routed from that commit, never from
  the working tree.
- A red check's fix thread is an ordinary thread. Its change takes the lane
  when the maintainer presses Apply. Only the maintainer closes a red; an agent
  or a script gets `403`.

### Calls made at build time

- **The run URL rides the verdict events, not `MainCheckStarted`.** The run
  exists only once the push started a workflow, minutes after the check must be
  recorded. The running snapshot carries it from the script's run line.
- **`MainCheckFailed` carries the tail and the run URL.** A red then rebuilds
  whole from events, fix-thread prompt included.
- **A fix landing closes the red, then the next check runs.** If that check
  fails, it is a new red with its own fix thread.
- **Recovery never starts a check.** It closes a red whose fix already landed,
  and abandons the check a restart cut off.
- **A lagged bus subscriber replays the landings it dropped.** It reads them
  back through `LagReplay`, so a dropped fix landing still closes its red.
- **A directory module, `engine/main_check/`,** where the plan said one file:
  the scheduler, runner, fix thread, recovery and snapshot each have tests.
- **Close takes the owner gate.** `require_owner_device` admits only a
  registered device, outside an app document. The machine-local token passed
  the first gate, which refused only agent subprocesses, so a script could
  close a red.
- **A deleted fix thread stays deleted.** A delete removes the thread's
  events, so it looked unprompted, and every restart opened it again.
  Recovery now reads `ThreadsDeleted` and leaves that red to Close.
- **Shutdown lets the check cancel its run.** The engine sends the group
  SIGTERM early in its teardown and waits up to 10 seconds for the script to
  exit. `main-check.sh` waits for its driver, its log reader ignores SIGTERM,
  and the driver's cleanup ignores SIGPIPE. Before, the reader died first and
  the cleanup died writing to it, so the run went on and the branch stayed.
- **The driver holds every message until Restore.** It starts before
  recovery, so a landing that came first started a check even while red. It
  shares `UntilRestored` with the landing lane. The runner slot also refuses a
  second child rather than overwrite the first.

## Alternatives considered

- **Gate each landing on the check.** That is a merge queue. At our 15% branch
  failure rate, batches spend their gain on bisection, and every landing would
  wait 17 to 40 minutes.
- **Run the check per push to mirror `main`.** Mirror `main` moves only at
  release, and a per-push trigger is what ADR 0382 rules out.
- **Retry a no-verdict check on a timer.** It hides an outage behind repeated
  runs, which ADR 0386 rejects for the same reason.
- **A fix thread per failing suite.** Two threads would chase one root cause,
  and both would land fixes the other did not see.
- **Let the fix thread close its own red.** The red's purpose is to hold checks
  until a fix lands. An agent that could close it could skip the fix.
