# 0386: E2E runs on GitHub by default; --local, local-only flags or an unmet precondition keep it on this host

- **Status**: Accepted. Amends ADR 0382, which made GitHub mode opt-in.
- **Date**: 2026-10-07

## Context

ADR 0382 let any e2e script run on GitHub's runners with `--github`. The
local run stayed the default, so every agent that ran e2e still competed for
this host's memory and queued behind the e2e lock. The maintainer asked for
the reverse: run on GitHub whenever that is possible, and run individual
specs locally now and then.

## Decision

**Every e2e script hands off to GitHub mode unless the run must stay here.**
`e2e_github_handoff` (`scripts/lib/e2e_github.sh`) makes the call, once, for
all five scripts. A run stays here when any of these holds:

- `--local` is passed.
- A flag that only means something on this host is passed: `--no-reset`,
  `--headed`, `--ios`, `--device`, `--screenshot`, `--pwa` or `--packaged`.
  So are cargo arguments to the wasm or embedder suite.
- A variable that changes what the run tests or produces is set: any
  `*_SHOTS` screenshot switch, `LUCIDOS_E2E_PACKAGED`, `LUCIDOS_E2E_DEBUG`,
  `LUCIDOS_E2E_WEBKIT_PHASE` or `LUCIDOS_E2E_WEBKIT_CHUNKS`. The driver
  forwards no environment, so GitHub would ignore it and report green.
- A static precondition fails: no release libraries (a public-mirror
  checkout), uncommitted changes, no `lucidos` remote, or no `gh`. The script
  prints the reason and runs locally.
- `LUCIDOS_E2E_LOCAL=1` is inherited.

A local decision exports `LUCIDOS_E2E_LOCAL=1`. The runner shard and the
driver's local leg set it too, so no nested e2e script ever hands off again.

`--github` stays. It skips the fallback, so an unmet precondition is a hard
error rather than a quiet local run. Combined with `--local` or a local-only
flag or variable, it is refused.

## Rationale

**GitHub is the cheaper place for a full verdict.** It runs 20 jobs at once
on fresh VMs, needs no e2e lock, and takes no memory from this host. Those are
ADR 0382's reasons. They apply to every run, not only the runs someone
remembered to flag.

**The fallback keeps the default safe for everyone.** A contributor's
mirror checkout has no release libraries, so it runs locally exactly as
before. An agent iterating on uncommitted changes gets a local run of what it
is editing, rather than a run of the last commit.

**A runtime failure is not a fallback.** A refused push or a run that never
starts is exit 76, no verdict. Turning that into a multi-hour local run would
surprise the caller and bring back the memory pressure the default avoids.

**One environment variable, not a flag on each nested call.** `e2e.sh` calls
its sub-scripts, the driver calls `e2e-browser.sh`, and the runner calls all
four. An inherited mark covers every one of those paths, including any added
later.

## Consequences

- An agent that runs `./scripts/e2e.sh` from a committed tree gets a GitHub
  run. It is a long wait, so it still runs as a background task.
- Fast local iteration needs `--local` or `--no-reset`. The `run-e2e` skill
  says so.
- The specs that need a logged-in Claude Code still run here, as the local
  leg, under the e2e lock.
- ADR 0382's four limits are unchanged: nothing deploys, no workflow holds a
  secret, nothing triggers per push or PR, and `/harden` stays the gate.

## Alternatives considered

**Keep a filtered run (`-f <spec>`, an API filter) local by default.** It
offered faster single-spec iteration. The maintainer chose GitHub for every
run that can go there, with `--local` as the explicit escape.

**Fall back to local on any GitHub failure.** Rejected for the reason above:
it hides an outage behind a long local run.

**Make `--github` the only way in, and change every caller.** Rejected: the
callers are skills, rules, the engine prompt and the nightly. One default in
one function cannot drift the way a dozen call sites can.
