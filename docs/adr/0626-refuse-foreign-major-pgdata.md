# 0626: The gateway refuses a pgdata from another Postgres major and leaves it untouched; releases pin the bundled major until a cross-major path exists

- **Status**: Accepted
- **Date**: 2026-11-07

## Context

The embedded cluster's binaries can open only a data dir from their own
Postgres major. When `PG_VERSION` named another major, the gateway stopped any
orphan, renamed `pgdata` to `pgdata.foreign-<major>-<ts>` and ran a fresh
`initdb`. Lucidos has no cross-major path (`pg_upgrade`, or dump and restore).

So the first release to bump the bundled major would have booted every user
onto an empty cluster. Every workspace would look empty, with the data in a
renamed dir nobody knows about. PG 18.4 is bundled today, so this has not
fired yet.

ADR 0625 decision 13 settles the policy: no release changes the bundled major
without a cross-major path. This ADR records the guard that lands ahead of the
rest of that plan.

## Decision

1. `ensure_embedded_cluster` refuses a data dir from another major with a
   terminal `ProvisionError` and touches nothing: no stop, no rename, no
   `initdb`. The message names the path and both majors, says the data is
   untouched, and names the way back. Newer data has one: the release that
   bundles its major. Older data has none, since no shipped release bundled a
   major below 18, so the message points at a backup or a dump instead.
2. `scripts/lib/release_pg_major.sh` pins `BUNDLED_PG_MAJOR=18`. Both build
   paths (`build-headless.sh`, `build-dmg.sh`) refuse any `PG_VERSION` of
   another major before they build. `release_pg_major_test.sh` fails if the pin
   moves.

## Rationale

**A refusal loses nothing, a repair loses everything.** An empty cluster boots
and looks healthy, so the user meets an empty app and no error. A refusal shows
on the boot splash and the picker, and the data sits where it was. When newer
data meets an older release, reinstalling the newer one opens it again.

**Terminal, because no retry can change the major.** A transient verdict would
retry with backoff and then say the same thing.

**Two gates, because each catches what the other cannot.** The release guard
stops a bump at build time, before any user sees it. The gateway refusal still
covers what no build sees: a hand-restored data dir, or a user who runs an
older release over a newer one's data.

## Consequences

- A data dir from another major blocks its workspaces until the user runs a
  release on that major. That is the intended trade.
- Raising `BUNDLED_PG_MAJOR` is the cross-major change itself. It ships with the
  migration path, its tests, and an edit to the pin test.
- `pgdata.foreign-*` dirs an earlier build left stay where they are. Nothing
  creates new ones.
- This ends the third exclusion target of ADR 0153 decision 2, the dir
  `move_data_dir_aside` kept. A refused `pgdata` still holds the plaintext
  `credentials` table, so the backup exclusion runs before the major check.

## Alternatives considered

- **Keep the move-aside and `initdb`.** Recoverable by hand, but the user sees
  an empty app with no error. Rejected: it reads as data loss.
- **Run `pg_upgrade` automatically now.** It needs the old major's binaries in
  the bundle and a tested rollback. That is the cross-major path ADR 0625 leaves
  open, not a guard.
- **Pin the major only in the gateway.** The gateway reads the bundled major
  from the binaries at run time, so it cannot tell a bump from an old data dir.
  Only a build-time pin stops a bump before it ships.
