# 0669: Checkout builds compile with a local-release cargo profile; shipped builds keep release

- **Status**: Accepted
- **Date**: 2026-11-12

## Context

An optimised engine built from a checkout used cargo's `release` profile. That
profile is not incremental and compiles each crate in 16 codegen units. Nearly
all of the build time is the `lucidos_engine` crate, and every commit on `main`
changes it. So sccache never helps with it, and each build recompiles the whole
crate from scratch.

Measured on the paired Linux cloud box (8 vCPU arm64, 15 GB RAM, 8 GB swap):

| Build, `release` profile | Wall time | Peak rustc RSS |
|---|---|---|
| Cold | 17m07s | 10.1 GB |
| One-line change in `lucidos_engine` | 11m49s | 11.0 GB |

The box follows `main`, so it pays the second row on every commit. An earlier
build was OOM-killed at 12.7 GB.

## Decision

Every optimised build made from a checkout compiles with a new cargo profile,
`local-release`. Shipped builds keep `release`. The launch dir keeps its name,
`.launch/release/<variant>/`, and one mapping pair in `scripts/lib/workspace.sh`
converts between the cargo profile and the launch profile.

## Rationale

- **`local-release` inherits `release`.** Our crates build incrementally across
  256 codegen units at opt-level 3. Dependencies keep opt-level 3, 16 codegen
  units and no incremental, exactly as release builds them. So their sccache
  entries and their runtime speed do not change.
- **It halves the build the box pays on every commit.** Measured on the box,
  same commits, wall time and peak summed rustc RSS:

  | Build | `release` | `local-release` |
  |---|---|---|
  | Cold | 17m07s, 10.1 GB | 19m42s, 10.3 GB |
  | One-line change | 11m49s, 11.0 GB | 5m05s, 7.5 GB |
  | A real 20-commit `main` delta | 11m49s, 11.4 GB | 5m50s, 7.7 GB |

  The `lucidos_engine` library itself drops from 619s to 220s after a one-line
  change. A loaded Mac builds in a 6-core build slot. There the one-line
  rebuild drops from 10m00s to 3m56s, and a cold build rises from 11m40s to
  12m40s.
- **Runtime does not change measurably.** Three preflight boots per binary on
  the box take 1.30s warm for both profiles. The binary is 16% larger: 216 MB
  against 187 MB.
- **The launch dir names what a build is for, the cargo profile how it
  compiles.** `.launch/debug`, `.launch/release` and `.launch/e2e` are the
  launch profiles. Cargo `local-release` publishes to launch `release`, and a
  `--publish-to` path under `.launch/release/` builds `local-release`.
- **The mapping is a bijection.** Cargo `release` has no launch dir, so ADR
  0022 still holds: a launch dir is written by one cargo profile only.
  `launch_profile_for_cargo_profile` errors on any profile outside the map.
- **Running installs switch with no relaunch.** The box and every source
  install started with `-r` or `run.sh` launch from `.launch/release/`. Their
  next Apply or follow-main build reads the profile back from that path, so it
  already compiles `local-release`.

## Consequences

- `RELEASE=1` means `local-release` wherever a checkout build reads it,
  including local e2e runs. The e2e engine cache keys on the profile, so its
  first run after this change misses once.
- A checkout gains `target/local-release/`, about the size of
  `target/release/` plus the incremental cache. The first build compiles our
  crates cold, and a cold build takes about 15% longer than with `release`. `target/release/` stays on disk until someone deletes it.
- An incremental build can produce slightly slower code than a whole-crate
  build. The boot measurement shows no difference, and shipped artifacts do
  not pay it either way.

## Alternatives considered

- **A new `.launch/local-release/` dir.** Rejected: every running install
  launches from `.launch/release/`. Its gateway spawns engines from that path,
  and its follow-main builds publish back into it. A new dir would keep each
  one on the slow profile until someone relaunched it by hand.
- **Change `release` itself.** Rejected: the DMG, the headless tarball and the
  release-tarballs workflow build with it. Shipped binaries should keep the
  whole-crate optimiser, and a release build compiles cold anyway, so it gains
  nothing from incremental.
- **Opt-level 2 for our crates.** Rejected: on the box it saved 5% at most
  (one-line change 4m51s against 5m05s, `main` delta 5m34s against 5m50s).
  That is too little to give up the full optimiser.
- **A job cap.** Not needed: a `local-release` rebuild peaks below a `release`
  one, and the cold build peaks within 0.2 GB of it.
- **A faster linker (mold or lld).** Deferred, not rejected. Link time is a
  small part of the build next to codegen. A follow-up can judge it from the
  cargo timing reports.
