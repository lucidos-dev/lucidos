# 0024 — The release candidate IS the published artifact: one stripped tree, built once, tested, then promoted

- **Status** — Accepted
- **Date** — 2026-07-28
- **Amended by**: [0039: The public mirror's `main` is a linear release
  history](0039-the-public-mirror-is-a-linear-release-history.md). The
  "squashed-orphan commit" this ADR describes is no longer parentless: since
  2026-08-04 it carries the previous release's published commit as its single
  parent. Everything this ADR decides is unchanged, including the part 0039
  leans on hardest, that the rc IS the published object. The parent is
  therefore resolved in Phase A and baked into that object, never re-derived at
  promotion time.

## Context

Lucidos publishes to the public mirror `github.com/lucidos-dev/lucidos` as one
squashed-orphan commit per release. That commit is built from a **stripped
tree**: everything in `EXCLUDE_PATHS` removed (internal planning notes, the
release scripts themselves, the signing/event libs) and `WORKSPACES.md` swapped
for a public stub, then scanned by the deterministic private-data guard before
the irreversible push.

Before the mirror went public (2026-07-28), two defects in the surrounding
pipeline were invisible. Both surfaced the same day.

1. **The release-candidate branch bypassed the stripping entirely.** The
   documented RC-gate step was a raw `git push lucidos <ref>:rc/<version>`, which
   pushes the FULL tree. `rc/0.16.0` and a stale `rc/0.14.0` sat on the mirror
   carrying ~400 files under `docs/plans/`, `scripts/release.sh`,
   `scripts/release-to-lucidos.sh`, the `release_signing` / `release_events`
   libs, and an un-stubbed `WORKSPACES.md` — every category the squashed-orphan
   `main` correctly excludes. A branch on a public repo is public content; only
   the repo still being private kept this from being a live leak.

2. **The gate validated a tree that was never published.** Phase B
   (`release.sh --publish-verified`) constructed its *own* stripped tree from the
   release commit and force-pushed that as a fresh orphan commit. So the commit
   CI tested (`rc/<version>`) and the commit users got (`main` + `v<version>`)
   were different objects, built at different times, by two code paths that could
   drift. The build-once discipline already applied to the `.dmg` — the bytes you
   mount are the bytes that ship — did not apply to the source tree.

The two defects share a root cause: the stripping logic existed in exactly one
place (inline in the publisher) and every other path that needed a public tree
either skipped it or would have had to copy it.

## Decision

**One stripped tree per release: built once, scanned once, tested, then
promoted.** Publishing is a *promotion* of the validated candidate, not an
independent rebuild.

1. **One strip implementation** — `scripts/lib/release_tree.sh` owns
   `RELEASE_TREE_EXCLUDE_PATHS`, the `WORKSPACES.md` stub, the fail-closed
   private-data scan, and the orphan-commit builder. Both the rc push and the
   publisher call it; `release-to-lucidos.sh` no longer declares an exclusion
   list of its own. The lib and its test are in the exclusion list themselves —
   a file that spells out what is withheld from the mirror does not ship.

2. **Phase A puts the real thing on the mirror.** `--verify-build` builds the
   stripped tree, scans it, commits it, and force-pushes it to
   `refs/heads/rc/<version>`; records the SHA as `RC_COMMIT` in the verify-build
   state; and, after staging, (re)creates the `rc-<version>` prerelease with the
   staged DMG + updater `.sig`. Those two pushes are exactly what
   `install-smoke.yml` gates on (`push: rc/**` and `release: prereleased`).

3. **The rc commit is deterministic.** Author/committer identity and dates are
   inherited from the internal release commit, never read from the clock, so the
   same (tree, message, source commit) always yields the same object. A retried
   or resumed Phase A re-pushes the *identical* commit instead of silently
   invalidating the recorded SHA. An existing remote rc whose tree matches is
   adopted rather than replaced, so a green gate is never discarded for a
   cosmetic difference.

4. **Phase B promotes and never rebuilds.** It refuses — before the confirm
   prompt and before anything public — unless an `RC_COMMIT` was recorded, the
   mirror's `rc/<version>` still points at exactly it, the staging manifest's
   `source_commit` equals the worktree HEAD, and every staged artifact's sha256
   still verifies. Then that same commit object is force-pushed to `main` and
   tagged. The rc branch and prerelease are deleted afterwards.

5. **The private-data guard runs at every push to the mirror**, not only at final
   publish — the rc branch is public the moment it exists. Both failure arms are
   fatal: a hit refuses, and a denylist that cannot load refuses (a disarmed
   guard must never read as "clean").

The rc branch push happens **before** the DMG build rather than after. It needs
only the release commit, so the slow clean-machine source-install legs run
concurrently with the local build instead of after it — and it moves the
fail-closed guard ahead of a 40-minute build rather than behind it.

## Alternatives rejected

**Keep the raw branch push, just remember to strip it by hand.** This is what the
runbook said to do, and it is what failed: the manual form is ~8 lines of
plumbing (temp index, per-path `rm --cached`, stub blob, `write-tree`, scan,
`commit-tree`, push) that an operator under release pressure will shorten. The
whole point of `EXCLUDE_PATHS` is that no human has to remember it.

**Let Phase B rebuild the tree, and just assert the two trees are equal.** The
comparison would pass on identical *content* while still publishing a different
*object* than CI ran on, and it keeps two construction paths alive — the drift
this ADR exists to remove. Comparing is strictly weaker than not rebuilding.

**Ship the tree straight to `main` and treat `main` as the candidate.** That
makes every gate run a live release; a red gate would already be public. The rc
branch exists precisely so the validated artifact can be discarded.

## Consequences

- **What CI tested is what ships**, at the object level, for both the source tree
  and the DMG. The whole release is now build-once.
- **A moved rc is a hard refusal**, not a warning: if anyone re-pushes
  `rc/<version>` after the gate went green, Phase B stops and names
  `--push-rc <version>` as the way to re-run the gate.
- **A failed rc push aborts Phase A before the build** instead of after it. The
  worktree is left for inspection; nothing expensive has been spent yet.
- **A build that later fails can leave an rc branch on the mirror** for a version
  that never ships. It carries the same stripped tree the release would have, and
  the next `--push-rc` or Phase B removes it. Accepted in exchange for the CI
  overlap and the earlier guard.
- **`release.sh --push-rc <version>`** exists as the no-rebuild recovery for a
  failed push, a replaced candidate, or a verify-build state written before this
  change (which Phase B refuses, by design, rather than guessing).
- The legacy one-shot (`release.sh <version>`) is unchanged and has no rc gate —
  it still builds its tree from HEAD, now through the shared lib.

## See also

- `scripts/lib/release_tree.sh` — the strip, the scan, the deterministic commit,
  and `release_promote_preflight` (Phase B's complete refusal set).
- `scripts/lib/release_tree_test.sh` — offline coverage of all of the above.
- `.claude/rules/no-private-data.md` — what the guard exists to keep out.
- `.claude/rules/build-release.md` § "The release candidate IS the published
  artifact" and `docs/desktop-app.md` § "Build-once / verify-first /
  publish-verified".
- `docs/plans/2026-07-28-rc-is-the-published-artifact.md` — the implementation
  plan and its invariants.
