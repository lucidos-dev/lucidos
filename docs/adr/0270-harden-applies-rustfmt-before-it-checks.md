# 0270: /harden applies rustfmt before it checks, and no write-time hook: amends 0030

- **Status**: Accepted
- **Date**: 2026-09-24
- **Amends**: [0030: `make lint` gates rustfmt](0030-rustfmt-gate-after-a-one-time-sweep.md)

## Context

ADR 0030 made `make lint` fail on any file rustfmt would rewrite. `lint-fmt`
reports and never fixes, and `make fmt` is the separate fix. Nothing formats
Rust as an agent writes it.

So hand-written code that rustfmt would rewrap fails the gate inside `/harden`.
`/harden` runs `make lint && make test`, so the engine suite never starts. The
whole run has to go again, which costs minutes for one rewrapped line. That
happened on the branch that added this ADR.

## Decision

For a diff touching `.rs`, `/harden` Phase 4.5 runs `make fmt` before the
suites. If that rewrote anything, it commits the result as its own
`style: rustfmt` commit. That commit does not send `/harden` back to Phase 1.

`lint-fmt` keeps `--check`. Nothing formats at write time.

## Rationale

**`/harden` is the one step every change passes through.** Apply runs it when
the marker is missing, and Claude Code and Codex both follow the same
`harden.md`. A step there covers every agent.

**Formatting needs no review.** rustfmt is deterministic and changes no
behaviour. Re-running Phase 1 over its output would only re-read the same code.
The step commits only from a clean tree, so the commit holds rustfmt output and
nothing else.

**The check still earns its place.** It proves the step ran, and it still
catches a change that reaches `main` some other way.

## Consequences

**Kept.** ADR 0030's stock defaults, no `rustfmt.toml`, and the `--check` gate.

**Given up.** Intermediate commits on a branch may be unformatted until
`/harden` runs. Nothing reads them in between, since Lucidos merges at Apply.

**A new commit on hardened branches.** A branch whose code was unformatted gains
one `style: rustfmt` commit.

## Alternatives considered

**A PostToolUse hook that formats each edited `.rs` file.** Code would never be
unformatted, even between commits. Rejected:

- It covers Claude Code only. Codex has no hooks, so the gate would still fail
  there.
- It rewrites a file under the agent, which must then re-read before its next
  edit.
- It adds a second mechanism to maintain beside the `/harden` step.

Jev (`jev-1.13.0`), asked the same question, put the `/harden` step alone at
0.61 against 0.33 for both.

**The hook and the `/harden` step together.** Rejected: the step alone already
prevents the failure, so the hook only buys tidier intermediate commits.

**Make `lint-fmt` fix instead of check.** Rejected: a lint target that rewrites
files hides the drift it exists to report, and the nightly would commit nothing.
