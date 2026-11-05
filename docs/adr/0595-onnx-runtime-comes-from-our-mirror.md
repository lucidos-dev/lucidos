# 0595: ONNX Runtime comes from a Lucidos mirror at build time, not only parcel.pyke.io

- **Status**: Accepted
- **Date**: 2026-10-28

## Context

`fastembed` pulls in `ort-sys`. Its build script downloads a static ONNX Runtime
from `parcel.pyke.io`, with one attempt and no second source. The v0.52.2
release lost three CI legs to it. A pyke outage blocks every from-source
install, every fresh contributor checkout, and every release. The default
`curl | sh` and DMG installs never compile, so they are not affected.

Plan: `docs/plans/2026-10-28-onnx-runtime-mirror.md`.

## Decision

Vendor `ort-sys` and patch its fetch. It tries a Lucidos-owned mirror first,
then pyke, with retries. The mirror is release assets on
`lucidos-dev/onnxruntime-mirror`.

Every attempt is verified against the upstream `dist.txt` SHA-256. The patch
removes the pkg-config probe, so every build links the pinned static library.
CI blocks `parcel.pyke.io` to prove the mirror suffices.

## Rationale

- **Integrity is already pinned; availability is not.** The trust chain is the
  `Cargo.lock` checksum, the crate, its `dist.txt` SHA-256, then the archive. The
  patch leaves `dist.txt` byte-identical and derives the mirror URL from it.
- **Only a patched build script covers every cargo invocation.** Plain
  `cargo build`, rust-analyzer and coding-agent sessions all reach it with no
  environment plumbing.
- **The mirror is a separate repo.** The desktop updater reads
  `releases/latest` on `lucidos-dev/lucidos`, and a published release there
  fires the dmg-verify trigger.

## Consequences

Each `ort` bump re-vendors the crate and uploads the new archives before
release. The release preflight enforces the upload.

The vendored copy is a temporary measure. It goes when an `ort-sys` release
lets a build name its download origin. The hosts-block leg passing with the
patch removed verifies that.

## Alternatives considered

- A wrapper setting `ORT_LIB_LOCATION`: plain `cargo` would still need pyke,
  and a changing env var rebuilds the engine.
- Pre-seeding the ort-sys cache: couples us to a private layout.
- `actions/cache` alone: misses still fail, and hits hide the regression.
- Retry alone: does nothing in an outage.
- `load-dynamic`: moves the failure to run time and adds a dylib to sign.
- Archives in git: 80 MB in the public mirror forever.
- Dropping `fastembed`: a product change, not a build fix.
- Hosting on `lucidos-dev/lucidos` releases: risks the updater's `latest`
  endpoint and trips the dmg-verify release trigger.
