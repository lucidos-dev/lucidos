# 0690: Confinement tests fail closed and run outside the agent sandbox, only under an owner permission

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0645](0645-agent-subprocesses-run-in-an-os-sandbox.md)

## Context

ADR 0645 runs every coding-agent session inside the OS sandbox. On macOS that
is a seatbelt, and a seatbelt never nests: under any outer profile, even
`(allow default)`, an inner `sandbox_apply` fails with EPERM.

The *confinement tests* apply their own profile to prove what it denies. These
are the 8 tests in `crates/lucidos-agent-sandbox/tests/enforced.rs` and one
python test in `runtime/python.rs`. Each one probed, printed `skipped:` and
returned. So a sandboxed session's `/harden` reported confinement as proven
when nothing ran. The owner ruled that tests which cannot run in the sandbox
are not acceptable.

## Decision

1. **A confinement test never passes without the real sandbox.** A failed
   probe panics with its reason. No skip, no early return.
2. **Confinement tests leave the ordinary suites.** This includes the two
   real-Chrome browser tests: Chrome applies its own renderer seatbelt, which
   cannot nest either. They remain no-ops on hosts without Chrome. Each carries
   `#[ignore = "confinement test: ./scripts/confinement-tests.sh runs it"]`.
   Only that script runs them, with `--ignored`.
3. **The script picks its route by a real seatbelt trial**, never by
   `LUCIDOS_AGENT_SANDBOXED`. Linux, or a macOS trial that applies, runs in
   place. A refused trial inside a coding-agent thread asks the engine. A
   refused trial anywhere else exits non-zero and says why.
4. **The engine runs the tests outside, under an owner permission.** The
   action is `run-confinement-tests` (ADR 0664). Without a live grant the
   engine raises a permission request: *Allow once*, *Always allow* or *Don't allow*.
   An agent can never pick *Always allow*.
5. **The engine owns the command.** `POST
   /api/v1/threads/:thread_id/confinement-tests` takes no command text. It runs
   `./scripts/confinement-tests.sh --here` in the thread's worktree as a polled
   background task. Only the thread's own agent may call it.
6. **A bare spawn needs a permission in hand.**
   `Sandbox::outside_for_confinement(&PermissionRow)` is the only production
   way to build an unsandboxed `Sandbox`.
7. **An engine that inherited a sandbox refuses the route.** Its children
   cannot leave the outer seatbelt.
8. **`/harden` runs a `confinement` suite** when the diff reaches the
   `lucidos-agent-sandbox` crate, `runtime/agent_sandbox*`, `runtime/python.rs`
   or `runtime/browser/*`, or the script. Hardening never refuses to start
   because a seatbelt cannot nest; only the confinement script trials it.
   The suite never raises a card (`--hold-secs 0`). A pending owner
   answer, a Don't allow or no grant fails the suite with exit 75, never PASS.
   So does a host where the sandbox cannot apply, such as a Linux kernel
   without Landlock. One exception: a passing run of this thread, at the
   commit the clean worktree is at now, certifies the suite (decision 12).
9. **The card names the risk.** The run executes this branch's code with no
   sandbox, so it can read Lucidos's secrets. *Always allow* grants that for
   every future run in the workspace, until revoked.
10. **This is the only thing that runs outside the sandbox.** The confinement
    route is the sole caller of the constructor in decision 6, and a guard
    test pins both.
11. **The CLI holds the card.** `lucidos confinement-tests run` raises the
    card and keeps its own tool call open until the owner answers, then
    follows the run. Any later step on a thread overtakes a card and removes
    its buttons, so only a call that waits may raise one.
12. **A passing run certifies its own commit.** The engine records each
    engine-started run as `ConfinementRunStarted`, with the worktree's `HEAD`
    when the tree is clean. The no-card call answers `passed` and exits 0 when
    the thread's latest run exited 0 at the `HEAD` the clean tree is at now. It
    checks that before any grant, so it never spends an *Allow once* to redo a
    proven run. A new commit, an uncommitted edit or a later failing run voids
    it, and the agent cannot forge the rows. A foreground call still runs the
    tests.

    Clean means git's status finds nothing with the user's config overridden,
    and no path is marked skip-worktree or assume-unchanged. A clean run hands
    its commit to the script, which fails the run when the tree leaves that
    commit before it ends. The run is on record before the gate admits the
    next call, and a run that cannot be recorded is stopped.

## Rationale

**A pass that did not happen is worse than a failure.** It tells the owner the
sandbox is proven when nothing tested it. Failing closed makes the gap loud.

**Leaving the sandbox is the only way to test it on macOS.** No profile
nests, so a sandboxed session cannot test confinement in place. The owner
chose a standing grant as the expected answer, so one tap per workspace
covers every later run.

**The engine, not the agent, decides what runs.** A route that took a command
would be a general escape from the sandbox. A fixed script, a fixed worktree
and a permission row keep the escape exactly as wide as the suite.

## Consequences

- **The honest cost:** a granted run executes branch-built code outside the
  sandbox, as an opted-out session would. That code can read Lucidos's
  secrets. The card says so, and the owner can revoke the grant in Settings.
- **One *Allow once* covers the hardening round.** The agent's foreground run
  spends the grant, and its pass certifies the retry round at the same clean
  commit (decision 12). Commit before that run, because a dirty-tree run
  cannot certify. A later commit needs a new run, and so a new grant unless
  *Always allow* holds.
- `make test` and the crates suite no longer run the confinement tests. Only
  `./scripts/confinement-tests.sh` does.
- A macOS shell inside some other seatbelt, with no Lucidos thread, cannot run
  them. It fails with the reason.
- The end-to-end path needs a running engine built from this change, so it is
  checked by hand: a sandboxed session runs `/harden`, taps *Allow once*, and
  the retry round passes.

## Alternatives considered

- **Keep the skip.** Rejected: it reports confinement as proven when nothing
  ran, which is the defect.
- **Read `LUCIDOS_AGENT_SANDBOXED` to pick the route.** Rejected: the marker
  says what the engine meant, not what the kernel allows. Linux nests, and a
  shell in another seatbelt has no marker.
- **A general "run outside the sandbox" route.** Rejected: it would undo ADR
  0645 for any command an agent names.
- **Refuse `/harden` in a sandbox.** Rejected: every sandboxed session would
  lose its gate. The unmerged preflight branch takes this route and conflicts
  with this ADR.
- **Raise the card from the harden suite.** Rejected: the agent's next step
  overtakes the card while the suite runs, so the owner never sees live
  buttons.
- **Run the tests on a GitHub runner.** Rejected: GitHub Actions never gates a
  change, since `/harden` is the per-change gate.
