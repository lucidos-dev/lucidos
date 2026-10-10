# 0250: A moved main merges the release commit, so the local tag names what shipped; LucidosReleased fires only after main carries the bump

- **Status**: Accepted
- **Date**: 2026-09-23
- **Amends**: [0029](0029-a-release-tag-names-the-main-line-commit.md)

## Context

Two defects hit v0.39.2 and v0.39.3 on the same day.

**The site published before main had the bump.** `LucidosReleased` starts the
lucidos.dev publish, and the site builds `install.sh` from main. The event
fired from `release-to-lucidos.sh` (and from `run_publish_draft`) before
`release.sh` landed RELEASE, CHANGELOG.md and install.sh on main. On v0.39.3
the event fired at 05:06:37, the bump landed at 05:06:40, and the site built at
05:06:48 with `LUCIDOS_DEFAULT_VERSION="0.39.2"`. So `curl | sh` installed the
previous version until a manual republish. The Download button was right,
because the site rewrites it from the event.

**The local tag named commits the release never shipped.** ADR 0029 lands the
bump on a moved main by cherry-picking the release commit, and tags the
cherry-pick. That commit sits on top of everything main gained during the
build. The next release counts from the newest `v*` tag, so it read that work
as shipped. `--prep-preflight` said "nothing to ship" and Phase A warned "same
tree as v0.39.2", both false.

## Decision

1. **A moved main MERGES the release commit** (`git merge --no-ff`), never
   cherry-picks it. The local and `origin` `v<version>` tag names the release
   commit itself. It is an ancestor of main through the merge, so 0029's
   invariant holds.
2. **`LucidosReleased` fires from `release.sh` only, after the source side is
   settled**: the landing, the local tag and the `origin` push. That covers the
   one-shot, Phase B and `--publish-draft` in both states.
   `release-to-lucidos.sh --no-released-event` is how `release.sh` tells it not
   to emit. A standalone `release-to-lucidos.sh` still emits at its end.
3. **A landing failure still announces.** A conflict fails the run only after
   the emit. STILL OWED says `install.sh` on the site lags until main carries
   the bump, and prints the command that republishes it. A failed push of main
   to `origin` gets the same line, since a publisher reading `origin` would
   still see the old installer.
4. **The two existing tags get a recorded override**, not a move.
   `RELEASE_TAG_CUT_OVERRIDES` in `release_main_sync.sh` records the commit each
   was cut from, and every run re-verifies it.

## Rationale

The tag is what every "new since the last release" question keys on: Phase A's
ahead count, the deleted-files gate, `--prep-preflight`, and a human typing
`git log v0.39.3..`. Making the tag itself honest fixes all of them at once.
A marker beside a dishonest tag has to be read by each of them, and the human
never reads it.

A merge and a cherry-pick of the release commit run the same three-way merge,
based on the cut point. Main's tree is identical and so are the conflicts. Only
the history shape changes, and main already carries merge commits.

The event order is a property of one function, `settle_and_announce_release`,
which both publishing tails call. There is one emit site in `release.sh`, and a
test pins it.

## Consequences

- After a merge landing, `v<version>..main` lists the moved-main work and the
  merge commit. `--prep-preflight` already skips merges.
- `git describe --tags main` reports `v<version>-<n>-g<sha>` after a moved
  landing, where it used to report the bare tag. It still names the right
  release.
- The merge runs `--no-verify`, matching the cherry-pick it replaces, which ran
  no commit hooks. A hook must not strand an already-public release.
- A Mode 2 release refuses to squash onto a recorded tag: that tag's tree holds
  the unshipped commits. The refusal names the `--base` to use.
- The override list is a temporary measure (`docs/temporary-measures.md`). Once
  a release newer than v0.39.3 is tagged, neither entry can be the previous tag
  again.

## Alternatives considered

**Tag the release commit without merging it.** It names what shipped, but it
is not an ancestor of main. Every drift check then degrades to advisory, and
`--prep-preflight` finds no usable base.

**Keep the cherry-pick and record a separate marker.** Three consumers must
read the marker, and `git log v<version>..` keeps lying to a human. The
cherry-picked bump also still reads as new work unless every consumer filters
it.

**Move the v0.39.2 and v0.39.3 tags.** The released commits are not in main's
history, so moved tags would degrade every check. Making them reachable needs
an `-s ours` merge on main outside any release, and `origin` would need a
forced tag update. The override covers the one release that will read these
tags, with no ref rewritten.

**Emit from `release-to-lucidos.sh` after a callback.** It has no source side
to settle. Moving the emit to the script that owns the landing is simpler than
teaching the publisher about main.
