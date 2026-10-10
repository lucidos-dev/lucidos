# 0042: A GA release is a draft until every platform tarball is attached

- **Status**: Accepted
- **Date**: 2026-08-05

## Context

A GitHub Release used to go public incomplete. `release-to-lucidos.sh` created
the Release, attached the DMG + updater trio and emitted `LucidosReleased`
within seconds; the four per-platform headless tarballs landed 11 to 35 minutes
later, attached by a `release-tarballs.yml` run that started **because** the
Release had been published. Measured on v0.21.0 (published 15:49:28Z): the DMG
trio at 15:49:32, then 16:00:49, 16:05:12, 16:08:42, and `x86_64-apple-darwin`
at 16:24:14.

Two costs, both live:

- the advertised `curl -fsSL https://lucidos.dev/install.sh | sh` genuinely
  failed inside that window for whichever platform had not been attached yet,
  and the landing page routes Intel Mac users to exactly the tarball that is
  consistently last;
- `install-smoke.yml`'s front-door jobs had to blind-wait on a guessed budget.
  `FD_ASSET_WAIT_SECS` was raised 1800 -> 3600 s on 2026-08-04 after v0.21.0's
  Intel tarball landed 3m33s past the old ceiling.

The same publish also built everything twice. A `push: tags: ['v*']` arm and a
`release: types: [published]` arm fired two seconds apart (runs 30926097107 and
30926100020), building the identical four tarballs; only the second attached
anything.

## Decision

Push the tag, build the tarballs, attach them to a **DRAFT** release, and
publish. The release is complete at the moment it becomes public.

Concretely: `release-tarballs.yml`'s **tag-push** arm is the attaching one and
its `release:` trigger is gone; `release-to-lucidos.sh` creates the GA Release
with `--draft`, attaches the DMG + updater trio, waits for the four tarballs
plus their `.sha256` sidecars, publishes with `gh release edit --draft=false`,
and only then emits `LucidosReleased`.

## Rationale

The publish is the only moment in the pipeline that is both irreversible and
observable by strangers, so it is the right place to put the completeness
precondition. Everything downstream already assumed it: the site chain repoints
`lucidos.dev` at the new release, and the front-door verification then asserts
that a stranger can install it.

Three properties follow from the ordering rather than from care:

- **The tag push is the trigger, so the fix applies to the release that ships
  it.** A `push` event runs the workflow file from the pushed ref, not from the
  default branch, and the `v<version>` tag names the new release tree.
- **`LucidosReleased` cannot fire over a draft.** It is emitted after the
  publish, so the site can never advertise a release nobody can see.
- **Publishing the draft starts no build.** A draft fires no webhook at all, so
  the `release:` arm could not have started the build anyway; keeping it would
  only have meant the publish kicked off a second 35-minute run that
  re-attached over files already there.

The wait is bounded (90 minutes), watches the `release-tarballs` run and fails
fast when that run fails, and is fully resumable: nothing is public while it
runs, so an interrupted wait costs one command
(`release.sh --publish-draft <version>`). That matters because the wait is
longer than a coding agent's tool call, which is the same constraint ADR 0027
worked around for notarization.

## Consequences

- **The attach window is gone**, so `install.sh`'s failure message stops naming
  it, `FD_ASSET_WAIT_SECS` drops 3600 -> 300 s (a CDN-propagation backstop, not
  a build wait), and both front-door jobs drop `timeout-minutes` 105 -> 75.
  `front_door_gate_test.sh` caps the budget at 900 s so re-inflating the
  band-aid reds a test rather than a release.
- **Half the tarball builds disappear**, one run per release instead of two.
- **The publish is no longer instant.** `--publish-verified` now blocks for 25
  to 45 minutes, where it used to return in about a minute. The release is
  invisible for that time rather than visible and broken.
- **A draft cannot be resolved through `GET /releases/tags/{tag}`** (a draft has
  no tag ref), so the CI attach step pages `GET /releases` and matches
  `tag_name`, and `permissions: contents: write` becomes load-bearing for
  READING: GitHub lists drafts only to a caller with push access. This is the
  same trap ADR 0036 documents for `dmg-verify`.
- **An incomplete release needs a human.** `--allow-missing-tarballs` is the
  only way to publish one.
- One more state to know about: a draft that exists but was never published.
  `release.sh --publish-draft <version>` is its only consumer, and it is
  idempotent.

## Alternatives considered

**Keep publishing first and just wait longer in CI.** This is what the
1800 -> 3600 s raise did. It treats a real user-visible outage as a CI timing
problem: the front-door job stops going red, and the stranger whose `curl … | sh`
404s is no better off. Rejected as a band-aid, which is how it was described
when it landed.

**Publish first, then have CI publish a second "complete" signal.** Adds a
second source of truth for "is this release usable" and leaves the first one
wrong for half an hour. The download URL is the signal; nothing else is.

**Attach the tarballs from the release machine instead of CI.** They are built
on four native runners across two operating systems and two architectures. The
release machine has one of those four. Rejected for the same reason ADR 0034
gives for not signing them locally.

**Fail open when the backfill dispatch is refused (HTTP 404 / 422), publishing
the incomplete release rather than stranding a draft.** Proposed while planning
this change and **declined** on 2026-08-04. The two states are not symmetric: an
automatic publish is the irreversible half, because it fires the site chain and
puts download links on a page for URLs that do not resolve, while a stranded
draft is invisible, costs one more operator command, and keeps every recovery
route open. Publishing an incomplete release stays possible, but only as
something a person typed: `--allow-missing-tarballs`.

**Create the draft BEFORE pushing the tag**, so the release is certain to exist
when the run looks for it. It buys very little (the attach step runs at the end
of a 10 to 35 minute build, and the draft is created seconds after the tag
push) and it would reorder the idempotency and adoption logic that ADR 0039's
partial-publish recovery depends on. The attach step retries the lookup for
300 s instead, and fails loudly rather than skipping.
