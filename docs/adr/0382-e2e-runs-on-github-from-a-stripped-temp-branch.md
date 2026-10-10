# 0382: The e2e suites may run on GitHub, from a stripped e2e run branch of the mirror

- **Status**: Accepted. Widens the "GitHub Actions is release-only" rule and
  supersedes ADR 0175's rejection of hosted macOS runners. ADR 0386 makes
  GitHub mode the default. ADR 0388 moves the Claude Code specs that need no
  real session onto GitHub, against a fake.
- **Date**: 2026-10-07

## Context

The full e2e suite ran only on the maintainer's Mac. Three costs followed:

- **The nightly competed with the maintainer for memory.** ADRs 0175, 0176 and
  0177 tuned guards around it, and on 2026-10-03 the host still panicked.
- **One run at a time.** The e2e lock serializes every agent that wants a run.
- **A full verdict took a night.** The `mobile-webkit` project ran in memory
  chunks of 3 specs on one host.

The rule at the time said GitHub Actions verifies releases and nothing else.
Its reasons were sound for a per-change gate: Lucidos is not PR-based, so a
`push` result arrives after Apply has merged. A `pull_request` trigger never
fires. ADR 0175 cited that rule when it rejected hosted macOS runners.

GitHub's standard runners are free on a public repository. That allows 20
concurrent jobs, 5 of them macOS, each on a fresh VM.

## Decision

**`./scripts/e2e*.sh --github` runs the selected suites on GitHub's runners.**

- The driver strips the commit under test with the release's own
  `release_tree` library and scans it with the private-data guard.
- It pushes the result as a parentless commit to `e2e/<run-id>` on the mirror.
- A workflow triggered by that push runs the shards. The driver downloads their
  output and prints the same report as a local run.
- The branch is deleted when the run ends.

Four limits stay:

- **Nothing in `.github/workflows/` deploys** (ADR 0031).
- **No workflow that runs e2e holds a secret.** Its token is `contents: read`.
- **Nothing triggers per push to `main` or per PR.** E2E runs only when someone
  asks for one.
- **The per-change gate stays `/harden`.**

## Rationale

**The agent asks for the run, so the old objection does not apply.** The rule
rejected CI because a push to `main` reports too late. GitHub mode pushes an
e2e run branch before Apply and waits for the verdict. The result can come in
time.

**Free parallel VMs fix the memory problem.** No Mac can run 20 jobs at once.
Each shard starts on a fresh machine, so no compressor pool builds up and no
memory guard needs to stop it.

**macOS runners keep the coverage honest.** ADR 0175's real concern was the
WebKit port: the macOS build is what iOS Safari ships. `mobile-webkit` runs on
`macos-15` arm64 runners, never on Linux WebKit.

**The public-data boundary holds.** The pushed commit has no parent, so it
reaches no private history (ADR 0039). It has passed the same fail-closed scan
as a release, and it carries a generic author and a generated message. An e2e
run branch is public content (ADR 0024). The maintainer accepted that for stripped
trees.

**Holding no secret keeps the blast radius at zero.** A workflow with no
credential has nothing to leak to someone who edits it. So the 12 browser specs
that need a logged-in Claude Code stay on the maintainer's Mac. They run there
as a local leg of the same command.

## Consequences

- An agent can get a full e2e verdict without the e2e lock or local memory.
- The mirror carries short-lived `e2e/*` branches. The driver deletes its own,
  and each run sweeps finished ones older than a day.
- Release safety needs care in two places:
  - The release preflight counts only the release workflows' queued runs.
  - E2E branches never save a cache, so they cannot evict the release caches.
- A cold run builds the engine from scratch before tests start. A cache-seeding
  job on mirror `main` keeps the dependencies warm for every `e2e/*` branch.
- GitHub mode needs push access to the mirror and the release libraries. It
  works only from the maintainer's checkout.
- Whether `/harden` should run e2e through GitHub mode is a separate decision.

## Alternatives considered

**A dedicated always-on Mac for the nightly.** Keeps everything local and needs
no policy change. Rejected: it removes the load from the laptop but keeps one
run at a time. It also gives no parallelism and adds hardware to maintain.

**Private-repo hosted runners.** No public branch at all. Rejected on cost:
macOS minutes bill at about ten times Linux, and a daily full suite exceeds the
included minutes fast. The private repo's Linux runners also have half the
cores and RAM.

**An Anthropic API key in the mirror's secrets, so every spec runs remotely.**
Rejected for the reason ADR 0031 gives: a credential in a public repo's CI is
exposed to anyone who can edit a workflow. Each run would also bill real model
calls.

**Linux WebKit in a container.** Rejected again, for ADR 0175's reason: it is a
different port from the one iOS ships.

**`workflow_dispatch` instead of a push trigger.** Rejected: dispatch runs the
workflow file from the default branch. Mirror `main` moves only at release, so a
harness change could not be tested until the next release shipped it.
