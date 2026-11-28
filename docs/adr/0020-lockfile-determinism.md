# 0020 — Deterministic builds via committed lockfiles, enforced with `--locked` / `npm ci`

- **Status** — Accepted
- **Date** — 2026-06-30

## Context

Build reproducibility and supply-chain safety: we want a build to use exactly the
dependency versions we've vetted — including the deep transitive graph — and we
never want a "random" newer (possibly hijacked) version pulled in silently.

The question that prompted this: *should we stop using caret/range version
specifiers (`serde = "1"`, `vite: "^6.1.0"`) and pin every dependency to an exact
version in the manifests instead?* The intuition was that ranges = nondeterministic
"auto-updates."

## Decision

Keep idiomatic caret/range specifiers in the manifests. Treat the **committed
lockfiles** (`Cargo.lock`, root `package-lock.json`) as the single source of truth
for exact versions, and make every build consume them **strictly**:

- All `cargo build|test|check|clippy|run` invocations in `scripts/**` + `Makefile`
  pass `--locked` (and `cargo tauri build … -- --locked`).
- All npm install sites use `npm ci` (never `npm install`).

A dependency version changes only by a deliberate `cargo update` / `npm install
<pkg>` that updates **and commits** the lockfile.

## Rationale

- **The lockfile already pins the whole tree; the manifest can't.** A manifest only
  names the ~30 *direct* deps. The hundreds of *transitive* deps — exactly where a
  hijacked-package risk lives — are pinned *only* by the lockfile. Exact-pinning the
  manifests would give a false sense of safety while leaving the deep graph governed
  by intermediate crates' own ranges.
- **Ranges are not "auto-updates" when the lockfile is committed.** `cargo build` /
  `npm install`/`npm ci` install the locked versions; a newer upstream release is
  ignored until someone runs an explicit update. The caret is just the *permitted*
  range for that deliberate update.
- **`--locked` / `npm ci` close the only real gap: silent lock rewrites.** Plain
  `cargo build` / `npm install` *respect* the lockfile but *can* rewrite it on a
  manifest drift. The strict forms **fail the build** instead — fail-closed. `npm
  ci` also verifies each package's integrity hash, so a hijacked republish of an
  existing version fails to install.
- **Exact pins actively harm us.** They break transitive de-duplication (two deps
  wanting different patches of one crate can't unify) and they *block security
  patches*: in the Dependabot sweep that motivated this, `jwt-simple`'s
  `rand = "=0.8.5"` exact pin is precisely what prevented updating `rand` to the
  fixed `0.8.6`.

## Consequences

- **Keep:** idiomatic manifests, transitive de-duplication, one-shot security
  patching via `cargo update` / `npm update`, full-graph reproducibility, and
  integrity-hash verification on npm installs.
- **Give up:** a build now **errors** if a manifest and its lockfile drift (e.g. you
  edit `Cargo.toml`/`package.json` without regenerating the lock). That is the
  intended fail-closed behavior — regenerate + commit the lock, then build. This
  also enforces that coding-agent changes keep the lockfile in sync.
- `npm ci` wipes + reinstalls `node_modules` (no incremental install). It only runs
  on a genuine dependency change in the dev path, behind the existing
  frontend-running guard, so the cost is bounded.

## Alternatives considered

- **Exact `=x.y.z` pins in every manifest.** Rejected: adds no determinism over the
  committed lockfile, doesn't cover the transitive graph (the actual risk surface),
  breaks de-duplication, and blocks security patches (the `jwt-simple` `rand` pin
  above). This is the option the discussion started from; the reasoning above is why
  it lost.
- **Status quo (committed lockfiles, plain `cargo build` / `npm install`).**
  Rejected: builds are deterministic in practice but *can* silently rewrite the lock
  on drift, so there's no fail-closed guarantee and no npm integrity enforcement.
- **No lockfiles, pin manifests only.** Rejected outright: loses transitive pinning
  and reproducibility entirely.
