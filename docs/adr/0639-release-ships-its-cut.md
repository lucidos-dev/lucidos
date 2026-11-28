# 0639: A release ships its cut: Phase B publishes the validated rc whatever main did since; --cut pins the commit

- **Status**: Accepted
- **Date**: 2026-11-09
- **Amends**: [0250](0250-a-moved-main-merges-the-release-commit.md)

## Context

v0.57.0 was rebuilt five times because main kept moving during the release.
Each Phase A costs about 15 minutes of build, sign and notarize. The RC gate
then takes another 45 to 57 minutes.

The script never asked for those rebuilds. `release_promote_preflight` reads
only the mirror's `main` and `rc/<version>`, and Phase B already lands the bump
on a moved main by merge (ADR 0250). The rebuilds came from the release runbook.
It said to always release from current main HEAD and to re-fold when main moved.
Two repo comments repeated that policy.

A second gap sat beside it. The changelog is drafted from main at one moment,
and Phase A cuts at main HEAD at a later one. Commits landing in between shipped
with no changelog line. `--base` could not pin the cut, because it also turns
off the landing on main.

## Decision

1. **A release ships its cut.** The *release cut* is the commit Phase A builds
   the release commit on. Phase B publishes the validated rc object and its
   staged artifacts unchanged, whatever main did since. Commits on main after
   the cut ship in the next release.
2. **The landing is unchanged.** Phase B lands RELEASE, CHANGELOG.md and
   install.sh on main: a fast-forward, else a merge, else a hard failure after
   `merge --abort`. The local and `origin` `v<version>` tag names the release
   commit, whose first parent is the cut.
3. **The run names what it did not ship.** The final summary lists the commits
   on main after the release commit as "not in v<version>". That list is
   exactly what the next release counts as new.
4. **`--cut <ref>` pins a Mode 1 release to a commit.** The ref must be main or
   an ancestor of main (`release_cut_resolve`). It is refused with a PR number,
   with `--base`, and on any phase but the one-shot and `--verify-build`. It
   never sets `BASE_REF_OVERRIDE`, so Phase B still lands the bump.
5. **Pair `--cut` with the preflight.** Pass `--prep-preflight`'s JSON
   `head_commit` as `--cut`, so the changelog draft and the build name one
   commit.

## Rationale

The rc gate validates one object, and ADR 0024 already makes Phase B publish
exactly that object. Re-folding to chase main threw away a validated build to
ship work nobody had gated. Shipping the cut keeps every published byte gated
and makes the release length independent of commit traffic.

The landing already handled a moved main correctly. ADR 0250 merges the release
commit, so the tag names what shipped and the next release counts post-cut work
as new. Nothing in the landing needed to change, only the policy around it.

`--cut` is a new flag and not a reuse of `--base`. `--base` means "a tree off
main's line", which is Mode 2 and skips the landing. A pinned cut is still on
main's line, so it must land like any Mode 1 release.

## Consequences

- A release no longer waits on a quiet main. A late commit waits for the next
  release instead of forcing a rebuild.
- The compiled-input gate (`release_build_fingerprint.sh`) stays, as the safety
  net for a deliberate re-cut. It is no longer described as the default path.
- At publish time main is routinely ahead of the cut. Any publisher that reads
  the live checkout's working tree, rather than the `v<version>` tag, can serve
  post-cut files with the release. That publisher must read the tag. The
  maintainer's site and docs publishers do so.
- The release runbook changes: record `head_commit`, pass `--cut`, and never
  re-fold because main moved.

## Alternatives considered

- **Keep re-folding, but only rebuild when compiled inputs changed.** The
  fingerprint gate already does this. It still re-runs the 45 to 57 minute gate
  on every re-fold, and it still ships commits the changelog never saw.
- **An incremental fold-in path.** Cherry-pick late commits onto the rc and
  re-gate only the delta. It adds a second way to build a release, and the
  gate would still have to re-run on the new object.
- **Refuse to publish when main moved.** That is the policy that caused the
  rebuilds, written into the script instead of the runbook.
- **Reuse `--base` to pin the cut.** It turns off the landing on main, so the
  bump, the tag and `origin` would all be skipped.
