# 0617: Engine builds require delivery proof

- **Status**: Accepted
- **Date**: 2026-11-02

## Context

A successful background build could leave the running engine's published
executable unchanged. Publish failures were swallowed, and background builds
always selected debug/plain even when the engine ran a release or feature
variant. Recording such a build as Ready left users unable to switch.

## Decision

Ready requires an uncached read of the executable at the running launch path.
Its build id must differ from the running engine and cover the current HEAD.
Script exit 0 transitions through Verifying rather than proving delivery.

Background builds target the running `.launch/<profile>/<variant>/` directory.
Engine and gateway publish failures are fatal; CLI publication remains best
effort. A stale publication gets one fresh package build per HEAD, preserving
Cargo locks and the build-slot protocol. A second undelivered build fails with
a named cause and Retry. HEAD churn is bounded too. Self-heal must not start
another set of retries after failure for the same HEAD.

An unavailable delivery probe is unknown, so it reports a verification failure
without cleaning packages or claiming that publication failed.

If an artifact covers the build's starting HEAD but HEAD moved, retry
incrementally. An artifact covering neither HEAD still gets a fresh retry.
Progress updates and failure budgets obey the background driver's generation
check, so a superseded build cannot overwrite its replacement.
Manual launches finish their build and output generation before stopping the
serving engine, preserving availability when either step fails.

Build diagnostics open inside the paired window. Handing the endpoint to an
OS browser or Safari can lose the device's pairing cookie. The viewer fetches
bounded text with the existing API client and renders it as literal text.

This applies to source and dev installs on macOS and Linux, including release
and feature variants and co-located workspaces. Packaged installs do not
self-build and are outside this flow.

## Rationale

Exit status proves process completion, while the switch consumes an executable
at a particular path. Verifying that artifact catches both publication failure
and stale build stamps. One fresh retry can repair a stale artifact without
turning failure into an unbounded build loop.

## Consequences

The build result describes a verified artifact, rather than merely a completed
process. Users retain a Retry action after failures. A source engine launched
outside the published layout must use the checkout's published launch path
before background delivery is supported. No component kills Cargo or removes
its locks to force a retry.

## Alternatives considered

- Treat exit 0 as Ready: cannot establish that anything switchable was published.
- Require a terminal relaunch after an undelivered build: strands remote users
  and repeats the same failure on later background builds.
- Retry indefinitely: consumes build capacity without improving the evidence.
