---
paths:
  - "scripts/build*.sh"
  - "scripts/release*.sh"
  - "scripts/lib/build_*.sh"
  - "scripts/lib/stage_runtime*.sh"
  - "scripts/lib/headless_tarball*.sh"
  - "scripts/lib/install*.sh"
  - "scripts/lib/service*.sh"
  - "scripts/lib/release_*.sh"
  - "scripts/lib/tauri_signing_key.sh"
  - "scripts/lib/cargo_lock_holders_test.sh"
  - "install.sh"
  - "uninstall.sh"
  - "docker-entrypoint.sh"
  - "Dockerfile"
  - "Makefile"
  - "rust-toolchain.toml"
  - ".github/workflows/release-tarballs.yml"
  - ".github/workflows/install-smoke.yml"
---

# Build, Packaging & Installer

Building the engine and desktop app, lockfile determinism, the shared runtime
staging tree, and the `curl … | sh` installer. The dev-runtime / gateway /
frontend-serving half of the former `scripts.md` is
`.claude/rules/dev-runtime.md`.

**`paths:` above is an explicit list, not a `scripts/**` catch-all** — that's
what makes a dev-script edit skip this file. A new script under `scripts/`
therefore gets NO rule until its path is added here or to `dev-runtime.md`.

## Build

```bash
cargo build -p lucidos-engine --release    # Engine
cd crates/lucidos-app && cargo tauri build # Desktop app
./scripts/build-dmg.sh                      # macOS: self-contained .app + .dmg (bundled PG)
./scripts/build-dmg.sh --emit-tarball       # macOS: ALSO emit the SIGNED headless lucidos-<version>-<triple>.tar.gz + .sha256
./scripts/build-headless.sh                 # Linux + macOS: Tauri-free headless tarball for the HOST triple
./scripts/build-headless.sh --check         # validate the resource contract (offline)
```

Dev: native engine + Docker PostgreSQL. Production: single Docker container. Makefile: `make build`, `make test`, `make run`.

### GitHub Actions is release-only — there is no dev-loop CI

Every workflow in `.github/workflows/` exists to verify a **release or delivery
artifact**, and that is the complete list of what belongs there:

| workflow | fires on | verifies |
|---|---|---|
| `docs.yml` | `release: published`, manual | the mkdocs site deploy (`mkdocs build --strict`) |
| `install-smoke.yml` | push to `rc/**`, `release: prereleased/released/published`, manual, weekly + daily cron | clean-machine `install.sh`, the notarized DMG, the tarball install, the live `lucidos.dev` front door, and the RC front door's payloads |
| `release-tarballs.yml` | `v*` tag push, `release: published`, manual | the per-triple headless tarball build |

**Never add a workflow that compiles, lints, type-checks or tests the tree per
push/PR.** The repo is built and tested locally; the per-change gate is
`/harden` (`.claude/hooks/pre-push.sh` blocks a push without a FRESH marker, and
Apply runs it synchronously when the marker is missing). A CI gate could not
substitute even if you wanted one: Lucidos is **not PR-based** — Apply merges
the branch into `main` directly — so a `pull_request` trigger never fires and a
`push` trigger only reports *after* the change is already on main. A new
per-change check goes into `/harden` Phase 4.5's test-selection table
(`.claude/commands/harden.md`). This is the CLAUDE.md rule "We build locally —
GitHub Actions is RELEASE-ONLY"; the section here is its rationale and the
inventory it is measured against.

`install-smoke.yml`'s two crons are the only non-release *triggers*, and both are
deliberate — each verifies something **external** to the tree, which is why no
local run and no per-change gate could substitute:

- **weekly** (`0 4 * * 1`) re-runs the clean-machine install to catch drift in
  external toolchains (rustup, apt, Homebrew).
- **daily** (`0 6 * * *`) runs the `front-door` job: the advertised
  `curl -fsSL https://lucidos.dev/install.sh | sh` against the **live deployed
  origin**, on a fresh `ubuntu:22.04`. Every other job tests a tree or an
  artifact built from one; this tests what the site is serving right now, which
  regresses independently of any commit — on 2026-07-29 the Pages deploy
  published `install.sh` but not the `scripts/lib/*.sh` helpers a piped install
  sources, and because Pages soft-404s (landing-page HTML at status **200**)
  `curl -fsSL` succeeded and the installer sourced HTML as shell. Its first rung
  asserts every helper lib resolves to a payload with a `#!` shebang rather than
  `<`, with the lib names parsed out of the served `install.sh`.

A schedule trigger fires the whole workflow, so **every job guards on
`github.event.schedule`** to claim exactly one cron. Adding a cron without that
guard silently multiplies the run frequency of every other job in the file.

Both are still delivery verification, not build gates.

#### `front-door` — one job, a parameterised origin, two modes

The origin is **not** hardcoded. `FD_MODE` + `FRONT_DOOR` are resolved from the
event, and the rung logic is written once:

- **`full`** — all four rungs, ending in a real `curl … | sh` on a bare box.
  Fires on the daily cron and on `workflow_dispatch` (input `origin`, default
  `https://lucidos.dev`). The **post-publish** caller is a dispatch *by the site
  publisher*, not the `release: published` webhook: the Pages deploy does not run
  in CI — it runs on the maintainer's machine off a workspace trigger chain
  (`LucidosReleased` → DMG-link bump → `SitePublishRequested` → publisher →
  `SitePublished`) — so the webhook fires mid-deploy and would verify the
  *previous* origin, passing for the wrong reason. The publisher fires the
  dispatch itself once `SitePublished` lands.
- **`payload`** — rung 1 only, then stop green. Auto-runs on every push to
  `rc/**` against the **RC front door** (`https://lucidos.dev/rc`, libs at
  `/rc/scripts/lib/`), so the soft-404 class is caught before anything reaches
  the real path. Also selectable on a dispatch.

**Payload mode must never run the install, and this is not a gap to close.** An
RC `install.sh` bakes `LUCIDOS_DEFAULT_VERSION=<rc version>` and resolves its
tarball to `…/releases/download/v<ver>/…`, but during an RC **that tag does not
exist** — Phase A publishes only an `rc-<ver>` prerelease carrying the DMG +
updater `.sig`, and headless tarballs live solely on real `v*` releases. Wiring
the install in would 404 at the download step on every single run and the gate
would be permanently red. Nothing is lost: the bug class the RC gate exists to
catch is the soft-404, and rung 1 catches it entirely by fetching and sniffing
payloads. Rung 2 cannot substitute — it asserts over the *log of a real install*,
so it needs exactly the tarball that does not exist.

Three properties keep a payload-mode green honest, and all three are load-bearing:

- the lib base derived from the served installer must equal **exactly**
  `$FRONT_DOOR/scripts/lib` — a prefix match let the apex vacuously satisfy an
  `/rc` base — and a mismatch is **fatal** in payload mode (where rung 1 is the
  only rung) while staying a warning in full mode (where rungs 2–4 still drive
  the origin);
- on an `rc/**` push the served installer's baked `LUCIDOS_DEFAULT_VERSION` must
  equal the version in `rc/<version>`, so the previous RC's copy sitting at the
  same URL cannot pass the gate;
- the lib-name scrape, the `LUCIDOS_INSTALL_URL` parse and the version parse all
  **fail closed** — a parser that finds nothing must never report green.

The **full** mode is still deliberately **not** on the `rc/**` push: it tests
production, not the RC tree, so a live-site outage must never be able to block
cutting a release. Payload mode is the inverse — it gates the RC's *own* copy.

The `origin` dispatch input is treated as hostile: the job pipes what the origin
serves into a shell, so a validation step accepts only `https://host[:port][/path]`
over a strict character allowlist, normalises trailing slashes, and exports the
result under a *different* name (`FRONT_DOOR_INPUT` → `FRONT_DOOR`) so a skipped
validation leaves consumers with an unset variable under `set -u` rather than a
usable one. The origin reaches the `sh -c` launch as a positional argument, never
string-interpolated. The job keeps `permissions: {}`.

### Toolchain pin — `rust-toolchain.toml` is authoritative

The repo root carries a **`rust-toolchain.toml`** pinning the exact Rust
toolchain (`channel`, `profile = "minimal"`, `components = ["clippy",
"rustfmt"]`, `targets = ["wasm32-unknown-unknown"]`). rustup honours it for
every `cargo` invocation anywhere in the tree — including from `signers/*/`,
which is why the wasm32 target has to be declared here (`signers/build-all.sh`
builds each signer to `wasm32-unknown-unknown`; without the target the build
fails with ``can't find crate for `core` ``).

It is the sibling of the lockfile rule below: `Cargo.lock` pins *what* is
compiled, `rust-toolchain.toml` pins *what compiles it*. Both exist so the same
source produces the same result on a dev box, in CI, and in the nightly. It
matters most for **lint** — clippy's lint set is a property of the toolchain, so
without the pin the clean-build gate's definition of "clean" drifts with
whatever rustup default a machine happens to have (the 2026-07-26 nightly's
`CleanBuildPassed` concern 4).

- **Bumping the pin is its own commit.** Change `channel`, run `make lint` plus
  the engine suite, and fix everything the new lint set surfaces *in that same
  change* — never let a stable bump red an unrelated branch.
- **Don't install a toolchain ahead of the pin in CI.** A `rustup … --default-toolchain stable`
  step that runs before checkout downloads a toolchain the pin then discards.
  `release-tarballs.yml`'s container step installs rustup with
  `--default-toolchain none` and lets the first post-checkout `cargo` call
  materialise the pinned one. Keep that shape. (The dev loop needs no such
  care: it builds locally, where rustup reads the pin from the tree.)
- **`rustup show active-toolchain`** in a CI log is the cheap way to see which
  toolchain a build actually used; `release-tarballs.yml` prints it.

### Lockfile determinism — builds are fail-closed (ADR 0020)

The committed lockfiles (`Cargo.lock`, root `package-lock.json`) are the single source of truth for exact dependency versions — the whole tree, direct **and** transitive. Every build consumes them **strictly**, so a build **errors** rather than silently rewriting a lockfile on manifest drift:

- All `cargo build|test|check|clippy|run` in `scripts/**` + `Makefile` pass **`--locked`** (and `cargo tauri build … -- --locked`). `cargo install tauri-cli` already uses `--locked`.
- All npm install sites use **`npm ci`** (never `npm install`) — installs exactly the lockfile, verifies integrity hashes, errors on `package.json`↔lock drift. `ensure_npm_deps` runs `npm ci` from the workspace root (`install_root`), behind its existing fingerprint gate + frontend-running guard. (It lives in `scripts/lib/workspace.sh`; the `ENGINE_ONLY` / `ENGINE_BUILD_ONLY` paths deliberately SKIP the install rather than fail — see `.claude/rules/dev-runtime.md` § "Engine-restart interaction".)

A dependency version changes ONLY via a deliberate `cargo update` / `npm install <pkg>` that updates **and commits** the lockfile. **Do not** "fix" a build failing with *"the lock file needs to be updated but --locked was passed"* by dropping `--locked` / reverting to `npm install` — regenerate + commit the lockfile instead. Manifests keep idiomatic caret/range specifiers (NOT exact `=` pins — see ADR 0020 for why exact-pinning is the wrong tool).

### CC worktrees get `node_modules` at spawn — don't reinstall it

A CC session must NOT run `npm install` / `npm ci` in its worktree "because `node_modules` is missing" — the engine provisioned it before the session started, and the reinstall is pure waste. Provisioning lives in `crates/lucidos-engine/src/engine/agent_session/run_session/spawn_context.rs` (`node_modules_setup::{has_install_marker, member_node_modules_links}`): for every **Lucidos-source** thread (NOT external-repo, NOT app-coding-agent), on spawn the engine **hardlinks** main's installed trees into the worktree (`cp -al`, ~1–2s, zero disk; `cp -a` fallback across filesystems), falling back to a cold `npm ci --prefer-offline` only when main itself has no install to copy. It links **two kinds of tree**: the hoisted worktree-ROOT `node_modules`, **and** each npm workspace-member `node_modules` that exists in main (`NPM_WORKSPACE_MEMBERS` = the root `package.json` `workspaces`; only `crates/lucidos-app/node_modules` has its own tree today — `packages/lucidos-sdk` fully hoists).

Two surprises that make a session *think* something's missing when it isn't:

- **A few deps live ONLY in the member tree, not the root.** Most deps hoist to `<worktree-root>/node_modules`, but an **un-hoistable** package sits in the member's nested `node_modules` — notably **`vitest` is at `crates/lucidos-app/node_modules/vitest`, NOT the root** (its 4.x tree conflicts with a root-hoisted version, so npm nests it; every `@vitest/*` path key in `package-lock.json` is `crates/lucidos-app/node_modules/...`). A root-only `ls node_modules/vitest` reports "missing" while `npm test` / `tsc --noEmit` resolve it fine. (This was the "Cannot find module 'vitest'" / `vitest: command not found` breakage before member trees were linked.)
- **The member tree carries NO `.package-lock.json` marker.** npm writes that marker only at the install ROOT, never into a member's nested `node_modules` (main's `crates/lucidos-app/node_modules` has real packages but no marker). So `has_install_marker` is the right check for the root tree only; the member link is gated on the source dir *existing* instead.

Verify the root tree with `ls <worktree-root>/node_modules/.package-lock.json` and the member tree with `ls <worktree-root>/crates/lucidos-app/node_modules/vitest`.

Why it matters beyond wasted minutes: a redundant `npm ci` saturates disk I/O and drove the mobile-webkit shard-contention wedge (`docs/plans/2026-06-27-mobile-webkit-shard-contention.md`), and a bare `npm install` would rewrite the committed `package-lock.json` — the exact determinism violation the section above forbids (ADR 0020). Frontend tests (`npm test`, `npx tsc --noEmit`) run against the provisioned tree as-is.

### Shared staging (`scripts/lib/stage_runtime.sh`)

The self-contained runtime tree — the 7 `RESOURCE_NAMES`: `lucidos-engine`, `lucidos-gateway`, `lucidos` (the CLI), `frontend`, relocatable **PostgreSQL 18 + pgvector** `postgres`, `sdk`, `system-knowhow` (engine-shipped reference docs, resolved by engines via `LUCIDOS_SYSTEM_KNOWHOW_DIR`) — is staged by ONE shared library that both build paths source:

- `stage_runtime_triple` — target-triple resolution from `uname`.
- `stage_runtime_fetch_postgres` — the theseus-rs PG18 + pgvector fetch/compile recipe; the same code resolves the macOS `*-apple-darwin` and Linux `*-unknown-linux-gnu` relocatable Postgres asset by triple. The `PG_SYSROOT` override applies only on a Darwin host; Linux uses system gcc.
- the frontend/binary builds, and the 7-resource `stage_runtime_assemble`.

Pure helpers are offline-tested by `scripts/lib/stage_runtime_test.sh`. The `lucidos` CLI is **load-bearing**, not a convenience: the engine resolves it as a sibling of `lucidos-engine` (`find_lucidos_cli_dir`), or by absolute path via `LUCIDOS_CLI_BIN` when the launcher stamps it (`desktop.rs::spawn_gateway` / `service_runtime_env_pairs`), to launch the Claude Code permission-prompt MCP server (`lucidos mcp-permission-server`). A bundle omitting it breaks every coding-agent thread on its first tool call — the engine now fails the CC spawn fast with a descriptive error rather than starting a doomed session (`resolve_lucidos_binary` in `crates/lucidos-engine/src/runtime/claude_code.rs`).

**Headless tarball — macOS signed (`build-dmg.sh --emit-tarball`).** In addition to the `.app`/`.dmg`, emits a per-platform `lucidos-<version>-<target-triple>.tar.gz` (the 7 `RESOURCE_NAMES`) plus a `shasum -a 256 -c`-compatible `.sha256` sidecar — the Docker-free, compile-free download artifact `install.sh` lays down (step 1 of `docs/plans/2026-06-30-installer-step1-headless-tarball.md`). Sourced from the SIGNED `.app` `Contents/Resources/` (not `bundle-resources/`, whose copies are never signed), so the Mach-O files keep their Developer ID signatures. Opt-in, applies to any build mode (no-op under `--check` / a build-less `--release-attach`); default behavior unchanged when absent.

**Headless tarball — Linux + macOS unsigned (`build-headless.sh`).** The Tauri-free build path (step 2 of `docs/plans/2026-06-30-installer-step2-linux-tarball.md`). Runs the shared staging for the **host** triple — no `cargo tauri build`, no `.app`, no DMG, no codesigning — then reuses `headless_tarball_emit` for the same `lucidos-<version>-<triple>.tar.gz` + `.sha256`. On **Linux** this is THE release build path; on **macOS** it produces an UNSIGNED tarball (use `build-dmg.sh --emit-tarball` for the signed one). It compiles natively, so `--triple` must equal the host — cross-arch artifacts come from the CI matrix's per-arch runners. Flags: `--triple`, `--out-dir` (default `.lucidos/release-staging/<version>/`), `--version` (default RELEASE → tauri.conf.json → 0.0.0), `--check`. Offline-tested by `scripts/lib/build_headless_test.sh`.

**Linux tarballs via CI (`.github/workflows/release-tarballs.yml`).** A `workflow_dispatch` + `v*`-tag-`push` matrix over the four target triples (`x86_64-unknown-linux-gnu` is the must-work entry; macOS x86_64 + Linux aarch64 are best-effort; `fail-fast: false`). Each entry runs `build-headless.sh` on a **native** runner — the Linux entries INSIDE an `ubuntu:22.04` container (the **glibc 2.35 floor**: a binary built on the raw 24.04 runner image refuses to start on Ubuntu 22.04 / Debian 12 / RHEL 9 with `GLIBC_2.3x not found`, and the same-machine tarball-smoke can't see it), guarded by an "Assert portability floor" step that fails the build if any staged binary references a `GLIBC`/`GLIBCXX`/`CXXABI` symbol version above that floor. Uploads the tarball + `.sha256` as **workflow artifacts only**. It does **NOT** auto-publish: never creates a Release/tag; the optional "attach to an existing Release" step is gated behind a manual `attach_to_release` input (default off) **and** a tag ref, and uses `gh release upload` (never `gh release create`). The signed macOS tarball still ships from the local `build-dmg.sh --emit-tarball` path; the macOS CI entries are unsigned, for parity/verification.

Packaging lives in `scripts/lib/headless_tarball.sh` (offline-tested by `headless_tarball_test.sh`); it copies with `ditto` on macOS (preserves embedded Mach-O signatures) and `cp -a` elsewhere (Linux runners have no `ditto`).

### The release candidate IS the published artifact (rc-first, ADR 0024)

A release is **one stripped tree, built once, tested, then promoted** — never a
tree that is validated and a second tree that ships.

**One strip implementation: `scripts/lib/release_tree.sh`.** It owns
`RELEASE_TREE_EXCLUDE_PATHS` (internal-only paths: `docs/plans/`, `release.sh`,
`release-to-lucidos.sh`, the `release_signing` / `release_events` /
`release_notes` libs, and `release_tree.sh` + its test — the lib withholds
itself; each excluded lib is sourced by a shipping script only behind an
`if [ -f … ]` with no-op stubs, or by an excluded caller only), the public
`WORKSPACES.md` stub, the fail-closed `release_tree_scan` (a hit refuses AND a
denylist that won't load refuses — see `.claude/rules/no-private-data.md`), and
the **deterministic** `release_tree_commit` (author/committer identity + dates
come from the release commit, so the same tree + message always yields the same
SHA — that is what lets a retried/resumed Phase A re-push the identical object).
`release-to-lucidos.sh` has no exclusion list of its own; a second copy is
exactly how the 2026-07-28 leak came back.

**Phase A (`release.sh --verify-build`)** arms both gate legs:

1. builds the stripped tree from the release commit, **scans it**, commits it as
   the orphan, and force-pushes it to `refs/heads/rc/<version>` — *before* the
   DMG build, so `install-smoke.yml`'s slow Ubuntu/macOS source-install legs run
   concurrently with the build and the private-data guard refuses a leaking tree
   before 40 minutes of build time, not after;
2. records `RC_COMMIT` in `verify-build-<version>.env` (alongside
   `SOURCE_COMMIT`, `PR_NUMBER`, `PR_TITLE`), pinned locally at
   `refs/release-candidates/<version>` so the object can't be GC'd between phases;
3. after staging, deletes + recreates the `rc-<version>` **prerelease** at that
   branch with the staged DMG + updater `.sig`, which fires the `dmg-verify` leg.

A **notarize resume** reaches both steps too. **`release.sh --push-rc <version>`**
re-arms the gate from the recorded release commit with no rebuild (a failed
push, a replaced rc, or a state file predating this flow). Both paths are
idempotent: an unchanged candidate is a no-op push, and an existing remote rc
whose *tree* matches is **adopted** rather than replaced, so a green gate is not
thrown away.

**Phase B (`release.sh --publish-verified`) promotes; it never rebuilds.**
`release_promote_preflight` refuses, before the confirm prompt and before
anything public, when: no `RC_COMMIT` was recorded, `rc/<version>` is gone from
the mirror, the mirror's rc **moved** (someone re-pushed ⇒ the gate result is
stale), `manifest.source_commit` ≠ the worktree HEAD, or any staged artifact's
sha256 drifted. Then `release-to-lucidos.sh --promote-rc <sha>` re-asserts the
unmoved rc, re-scans that commit's tree (the deterministic floor at the
irreversible push), and force-pushes **that same object** to `main` + tags it
`v<version>`, attaches the staged artifacts, and the rc branch + prerelease are
deleted.

The legacy one-shot (`release.sh <version>`, no phase flag) still builds its own
tree from HEAD through the same lib and has no rc gate. Offline-tested by
`scripts/lib/release_tree_test.sh` (strip coverage, self-exclusion, guard
fail-closed on both arms, commit determinism, all five preflight refusals, and
the wiring that keeps the promotion a promotion).

### Notarization is resumable — never a foreground `--wait`

Apple's notary service routinely outlives the process waiting on it, and the orchestration layer caps background tasks at **3600 s**, so a slow notarization can never be held in a foreground wait. `build-dmg.sh` therefore submits with **`--no-wait`**, **persists a resume handle before any waiting** (`<repo-root>/.lucidos/release-state/notarize-<version>.json` — submission id, absolute DMG path, its sha256 at submit time, source commit, timestamp; written atomically by `scripts/lib/release_notarize.sh`), then polls `notarytool info`. Losing the waiter costs a poll, not a rebuild — before this, a killed wait threw away a full cargo release build, ~134 inside-out codesigns, and a signed DMG, because the staple, the staging dir, the `manifest.json`, and the only copy of the submission id all died with the process.

- **Resume:** `release.sh --resume-notarize <version>`, or `build-dmg.sh --resume-notarize` directly. Re-running `release.sh --verify-build <version>` auto-promotes to a resume when a handle exists (and then no longer requires `-c` — that changelog is already committed). The resume reuses the EXISTING Phase A worktree and writes the `verify-build-<version>.env` state the killed run never reached, so `--publish-verified` works afterwards.
- **Adopt:** `build-dmg.sh --adopt-submission <uuid>` records an in-flight submission whose id was never persisted against the on-disk DMG, then resumes.
- **The resume gate is strict** (`release_notarize_resumable`): the DMG must still hash to what was submitted, AND the tree must still be on the recorded `source_commit`. The second half is load-bearing — the resuming run stamps `manifest.source_commit` from its own HEAD, so resuming on a moved tree would make the staging manifest claim a commit the DMG was never built from, and `--publish-verified`'s identity guard would pass on a lie. A build-grade run that finds a NON-resumable handle says why and rebuilds; an explicit `--resume-notarize` fails loud.
- **Terminal cases:** `Accepted` → staple (idempotent, via `stapler validate` fallback) + stage; `Invalid`/`Rejected` → print the notary log and refuse to stage; an id Apple doesn't recognise → say so and require a fresh submit (never silently re-submit). The handle is dropped once staging succeeds, so a later run can't resume a finished release.
- **Credentials are unchanged and load-bearing.** One `notarytool_run` wrapper resolves them for `submit`/`info`/`log` alike: App Store Connect API key first (`-i` only when `APPLE_API_ISSUER_ID` is set — required for Team keys, must be omitted for Individual ones), else `APPLE_PASSWORD` piped on **stdin**. Never `--password` in argv (world-readable via `ps`), never `store-credentials` (headless ⇒ "User interaction is not allowed"). `build_dmg_test.sh` asserts all of this against the notarytool call sites.

Offline-tested by `scripts/lib/release_notarize_test.sh` (the pure handle: round-trip, checksum/commit/missing-DMG refusals, UUID shape, notarytool JSON parsing) and the resume section of `build_dmg_test.sh` (flag parsing, the end-to-end gate, and the `release.sh` phase plumbing). Knobs: `NOTARIZE_POLL_INTERVAL` (30 s), `NOTARIZE_POLL_TIMEOUT` (7200 s — bounds the process, not the handle), `NOTARIZE_POLL_MAX_FAILURES` (5).

### The release does not wait on Apple — deferred DMG (ADR 0027)

Resumability stopped a slow verdict costing a *rebuild*; it did not stop it blocking the *release*. **`--defer-notarization` does.** Notarization gates exactly one artifact — the `.dmg` a browser downloads. The headless tarball and the updater trio (`.app.tar.gz` + `.sig` + `latest.json`) are never quarantined, so Gatekeeper never assesses them: existing users and `curl … | sh` installs are wholly unaffected.

- **Phase A** — `release.sh --verify-build --defer-notarization <version>` submits, persists the handle, and stages the **unstapled** DMG with `notarized: false` in the manifest. With an already in-flight submission it stages **without polling**, which is the rescue path for a Phase A stuck on a slow verdict. It does **not** create the `rc-<ver>` prerelease (`arm_dmg_gate_if_notarized`): `dmg-verify` asserts a stapled ticket, so arming a gate that must fail says nothing.
- **Phase B** — `--publish-verified` publishes with the *notarization-pending banner* on the Release body and **keeps** the worktree, staging, state file, notarize handle and submitted-bytes pin. Those are the attach step's only inputs; deleting them would strand a published DMG unstaplable.
- **Finish** — `release.sh --attach-notarized <version>` polls, staples, re-stages, `--clobber`s the asset in place, rewrites the body **after** the upload lands, dispatches `dmg-verify` against the published tag (`-f dmg_tag=v<version>`), emits `ReleaseDmgNotarized` (which bumps the site link), then runs the deferred cleanup.

**The pending state is a manifest field, never a flag.** `release_staging_is_notarized` is the single question every public-facing consumer asks, so the banner, the site link and the cleanup cannot disagree with the bytes. An **absent** `notarized` key means notarized (the pre-2026-07-29 writer staged only after `Accepted`), and `restage_manifest_for_commit` carries the value forward — a restamp that dropped it would launder a deferred staging into a clean-looking one. `--defer-notarization` is refused on `--release` / `--release-attach`, which upload in the same process where no banner can be composed.

**Two accepted costs, both documented at their sites.** The updater tarball is built pre-staple, so early auto-updaters keep an unstapled (still Developer ID signed) bundle forever — invisible in practice, and unreachable by a later re-issue. And an `Invalid`/`Rejected` verdict now lands on an already-public asset: pull it with `gh release delete-asset`, leave the banner up, patch-release the fix. Deliberately not automated.

Banner + changelog-section text live in `scripts/lib/release_notes.sh` — **one** extractor shared by the publish and the attach step, so the body the attach step rewrites is byte-identical to the one the publish wrote. Offline-tested by `release_notes_test.sh` (banner content, the compose that never touches `$NOTES_FILE`, and that latest.json's notes stay plain) and the deferred sections of `build_dmg_test.sh` + `release_staging_test.sh`.

## Installer (`install.sh` + `uninstall.sh`)

`install.sh` is the user-facing `curl … | sh` installer (steps 3 + 4 of `docs/plans/2026-06-30-installer-step3-download-and-run.md` + `docs/plans/2026-06-30-installer-step4-service-mode.md`). **Three modes:**

- **(default) download-and-run + register a service** — detect the host triple (the SAME `stage_runtime_host_triple` map the build scripts use — no divergent mapping), resolve the version, `curl` the prebuilt `lucidos-<version>-<triple>.tar.gz` + `.sha256`, **verify the checksum (mandatory, fail-closed)**, extract to the SHARED `$LUCIDOS_PREFIX/runtime/<stem>/`, then **register the bundled gateway as a user-level service** so it survives terminal-close + reboot and restarts on failure. The service runs `lucidos-gateway` directly with the SAME env `crates/lucidos-app/src/desktop.rs::spawn_gateway` sets (`LUCIDOS_GATEWAY_PG_BACKEND=embedded`, `LUCIDOS_PG_BIN_DIR`/`LUCIDOS_PG_LIB_DIR`, `LUCIDOS_ENGINE_BIN`, `LUCIDOS_STATIC_DIR`, `LUCIDOS_SDK_DIR`, `LUCIDOS_SYSTEM_KNOWHOW_DIR`, `FASTEMBED_CACHE_DIR`, `LUCIDOS_BOOT_WITHOUT_PROVIDER=1`, `LUCIDOS_PACKAGED=1`) — emitted once by the pure `service_runtime_env_pairs`, shared by the foreground launch + the plist + the unit. `--no-service` (`LUCIDOS_NO_SERVICE`) runs the gateway in the **foreground** instead (the step-3 behavior). No Docker/Rust/Node/clone/compile.
- **`--dev` / `--source` / `LUCIDOS_FROM_SOURCE=1`** — the legacy compile-from-source path, preserved verbatim (toolchain bootstrap, clone/update, `data/.env`, build + launch via `scripts/run.sh`). The only network/compile path; **always foreground** (never registers a service).
- **`--from-tarball <path>`** — install a LOCAL tarball (offline; e.g. from `build-headless.sh`). Verifies the adjacent `<path>.sha256` if present (fail-closed), warns if absent, extracts, and registers the service too (unless `--no-service`).

**Service = the GATEWAY only (ADR 0014).** The service supervises the gateway; the gateway provisions the embedded Postgres + spawns/supervises the engines itself — never a service per engine. The gateway ignores SIGTERM and stops gracefully on SIGUSR1 (`crates/lucidos-gateway/src/server.rs`), so the systemd unit sets `KillSignal=SIGUSR1` + `KillMode=process` (stop the gateway; leave engines + PG for a relaunch to re-adopt).

**Slug-keyed multi-instance.** Several gateways coexist as named *instances* (`--name <slug>` / `LUCIDOS_INSTANCE`, default `default`). The **port is a mutable property**, not the identity, so a re-run with a new `--port` moves an instance. Each instance owns `<prefix>/<slug>/` (registry + embedded PG + `fastembed/` + `logs/` + a `port` marker) and a slug-suffixed service id; the **runtime is downloaded once and SHARED** at `<prefix>/runtime/current`. Slugs `gateway`/`runtime`/`current`/`logs` are reserved (so a `--name` can't alias the dev gateway's `~/.lucidos/gateway` or the shared runtime). This is how a terminal install coexists with a dev gateway (5251) and the packaged `.app` (5252). **Service ids + paths:** launchd `com.lucidos.gateway.<slug>` at `~/Library/LaunchAgents/` (logs `<prefix>/<slug>/logs/gateway.{out,err}.log`); systemd `lucidos-gateway-<slug>.service` at `${XDG_CONFIG_HOME:-~/.config}/systemd/user/` (logs `journalctl --user -u lucidos-gateway-<slug>`).

**Port resolution (idempotent; port is changeable).** Pinned `--port P`: use P if free or already this instance's, else **fail closed** (a foreigner holds it). Bare on an existing instance: reuse its recorded `<data>/port`. Bare on a NEW instance: auto-pick the first free port from 5252 up (stepping around a running `.app`). After registering, a **health check** polls `http(s)://localhost:<port>/~/api/v1/health` (`LUCIDOS_HEALTH_TIMEOUT`, default 120s; `curl -k`, scheme follows the TLS opt-in) and fails loud with a logs hint if it never answers.

**TLS opt-in (`--tls-cert`/`--tls-key`, env `LUCIDOS_TLS_CERT`/`LUCIDOS_TLS_KEY`).** Both-or-neither, files must exist (fail closed). When supplied, the pairs are appended to the service/foreground env (`service_tls_env_pairs`) so the bundled gateway serves **https** — which is what gives a NON-localhost device a secure context (service worker, PWA install, web push all require one; plain http limits them to localhost). Works with `tailscale cert` / mkcert / CA certs. Engines still never see `LUCIDOS_TLS_*` (the gateway strips them — it terminates TLS, ADR 0014), and `restart_via_gateway` tolerates the scheme mismatch via `peer_scheme_order()`. Remote reachability is separate (`--bind` below, or Settings → System → Network access; loopback-only default unchanged). Like provider creds, TLS is baked from THAT run's flags — a re-run without them reverts the service to plain http.

**macOS CLT preflight (download / from-tarball paths).** `install.sh` probes `xcode-select -p` on Darwin and **warns (never dies)** when the Command Line Tools are absent: chat works, but coding agents / Apply / `run_python` shell out to git + python3, whose `/usr/bin` shims error until CLT is installed. The engine mirrors this at boot (`git_preflight` + `python_preflight` in `main.rs`, warn-only) and startup-augments its own process PATH with the common user-install bin dirs (`core::user_path::augment_process_path` — Homebrew, `/usr/local/bin`, `~/.local/bin`, npm-global; dedupe ⇒ no-op on a dev shell PATH) so bare-name tools (`claude`/`codex` fallbacks, chat bash/python shell-outs, stdio MCP servers) resolve under the launchd minimal PATH exactly as in dev. Agent children additionally get the bundled `LUCIDOS_PG_BIN_DIR` PATH-prepended (`spawn_env::agent_path_prefixes`) so the advertised bare `psql -c '…'` works inside coding-agent threads on a packaged install, mirroring what `workspace_script_env_vars` already did for chat bash/python tools.

**Manager detection + degrade.** macOS → launchd; Linux → systemd `--user` (probed via `systemctl --user show-environment`) + best-effort `loginctl enable-linger` (announced, never hard-fails). **No supported manager** (e.g. a container) → **degrade to a foreground launch** with a clear message, never fail.

**Post-extract validation + preflights.** `finish_install` runs the extracted `lucidos-gateway --build-id` once — the **execution smoke**, so a too-old glibc / wrong-arch tarball fails AT INSTALL TIME with a distro-floor message pointing at `--dev`, instead of an opaque service crash-loop. Then it warns — never fails — about missing host runtime deps: `git` (the engine shells out for every git op) and, on Linux, a system CA bundle (candidate list = `install_ca_bundle_candidates` in `install_common.sh`; rustls reads the system store for LLM/model/web-push TLS).

**Remote access (`--bind`) + unit-value escaping.** Default posture stays loopback + plain http; the final banners print the remote options (SSH tunnel and `tailscale serve` keep a SECURE origin — which web push + PWA require — with zero config), and the https half is the TLS opt-in above (`append_tls_env` is ADDITIVE, so the flag-less env block stays byte-identical to `spawn_gateway`'s contract). `--bind all|loopback|<IP>` (`LUCIDOS_BIND`) writes the machine-global `~/.lucidos/network.toml` via `service_write_network_toml` (byte-mirror of the gateway's own writer, preserves `[engine] inherit`) — **never unit env**, which would permanently shadow the picker's Settings → Network access knob (env beats the file). Invalid `--bind` values are refused up front (the gateway would silently fall back to loopback). systemd unit values are escaped via `service_systemd_escape_env` (`%%`, `\"`, `\\` — an API key with `%` used to reach the gateway mangled); launchd's twin is `service_xml_escape`.

**Uninstall.** `uninstall.sh` (and `install.sh --uninstall`, which delegates to it): `--name <slug>` removes one instance (a bare uninstall removes the sole instance, else lists), `--all` removes every instance, `--list` shows instances + ports. It stops + unregisters the service (both launchd + systemd artifacts that exist), gracefully stops that instance's engines + embedded Postgres, and **keeps all data unless `--purge`** (prints what it left). `--purge` deletes the instance data dir; `--all --purge` also deletes the shared runtime. The systemd unit FILE is removed **even when the user D-Bus session is unreachable** (bare ssh, no `XDG_RUNTIME_DIR`) so an "uninstalled" service can't resurrect at the next boot; in that case the possibly-running stack is left alone (a bus-less shell can't stop the gateway, and killing its engines would only make it respawn them).

**Shared logic, one source of truth.** install.sh **sources** `scripts/lib/{stage_runtime,headless_tarball,install_common}.sh` (triple/stem/URL) and `scripts/lib/service.sh` (service templating/detection) from `<self>/scripts/lib` when run from a checkout; when piped it **fetches** those small pure libs from the same ref (`${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib`, overridable via `LUCIDOS_LIB_BASE_URL`) — never re-implementing any map.

**A fetched lib is content-sniffed before it is sourced, fail-closed.** `curl -fsSL` plus a non-empty test cannot see a **soft-404**: an origin that answers an unknown path with its landing page and a **200** makes both checks pass, and `.` then executes HTML as shell. That shipped — a clean `ubuntu:22.04` running the advertised one-liner on 2026-07-29 died on ``stage_runtime.sh: line 1: `<!DOCTYPE html>` `` because the Cloudflare Pages SPA fallback served the landing page for `scripts/lib/*.sh`. `_source_libs` therefore rejects a payload whose first non-blank line opens a tag (`<!DOCTYPE`, `<html`, `<?xml`), naming the lib + origin and pointing at a checkout or `--dev`. The missing-file half was fixed at the publisher (it now uploads the libs beside `install.sh`); **the sniff is the defence in depth and stays** — a wrong or hijacked origin can still soft-404, and this is the one place unknown remote content reaches `source`. Covered by `install_test.sh` in both directions: an HTML payload for every lib is refused and never reaches the shell, and the real libs fetched over `file://` still install cleanly.

`install_common.sh` holds the pure URL/version/dir helpers; `service.sh` splits **PURE** helpers (identity, paths, plist/unit templating, manager DECISION + compose decision, env pairs, slug/port validation, port candidates, uninstall paths) from thin **EFFECTFUL** wrappers (launchctl/systemctl/curl/kill/pg_ctl calls, port probing, instance listing) — the offline tests exercise the pure ones and never the effectful ones.

**Version resolution — the baked default is what the public one-liner uses.** `install_resolve_version` takes `--version`/`LUCIDOS_VERSION` → the `RELEASE` file **next to the script** → the baked `LUCIDOS_DEFAULT_VERSION` in `install.sh`. A piped `curl -fsSL https://lucidos.dev/install.sh | sh` has no checkout and therefore no adjacent `RELEASE`, so **every public install lands on the baked constant** — RELEASE only reaches an install run from a checkout. That made a stale constant a shipped outage, not a cosmetic lag: 0.14.0 predates headless tarballs, so the advertised one-liner 404'd. Two things keep them in lockstep: `release.sh` rewrites the assignment (anchored to line start, failing loud if the pattern doesn't match) in the same step that bumps `RELEASE`, and commits `install.sh` with it; and `install_test.sh` asserts the parsed constant equals the repo-root `RELEASE`, so a hand-edit — or a removed substitution — fails a test rather than a user's install.

**One source of truth for the version — `RELEASE`, and enforcement.** Everything else DERIVES from it: `build.rs` reads it at build time, `release.sh` rewrites `install.sh`'s baked constant at release time, and the dev workspace's site publisher pins the landing page's download links at publish time. A second hand-maintained copy always drifts — the baked constant did (a shipped 404), and CONTRIBUTING.md and PRIVACY.md both announced the "0.9.x line" long after main left it. `scripts/lib/version_sources_test.sh` enforces this: it scans the tracked tree for the current version anywhere nothing keeps in sync, pins both halves of the install.sh mechanism (the equality AND release.sh's rewrite, so deleting the substitution fails now rather than at the next bump), and flags prose that announces which release line the project is on. Historical narration is deliberately exempt — CHANGELOG, `docs/plans`, `docs/adr`, and text explaining that "0.14.0 predates headless tarballs" are correct precisely because they don't move; only claims about the CURRENT version rot. Phase A runs the suite against the WORKTREE right after both bumps, so a stale literal fails the release instead of shipping. Three gotchas are documented at their sites in the suite: `git grep -E` is POSIX ERE where `\b` is **not** a word boundary (use `-w`), a pathspec of only `:(exclude)` matches **nothing** (needs a leading `'.'`), and macOS bash 3.2 has no `mapfile`.

**Layout.** `LUCIDOS_PREFIX` (default `$HOME/.lucidos`) → shared runtime at `<prefix>/runtime/lucidos-<version>-<triple>/` + a `<prefix>/runtime/current` symlink; per-instance data at `<prefix>/<slug>/` (override the single-instance data dir with `LUCIDOS_GATEWAY_DATA`). **Idempotent:** an already-extracted runtime for the target version isn't re-downloaded/re-extracted unless `--force` (`LUCIDOS_FORCE`). `--no-launch` (`LUCIDOS_NO_LAUNCH`) installs without starting or registering.

**Env/flags:** `--name`/`LUCIDOS_INSTANCE`, `--version`/`LUCIDOS_VERSION`, `--base-url`/`LUCIDOS_RELEASE_BASE_URL` (default `https://github.com/lucidos-dev/lucidos/releases/download/v<version>`), `--prefix`/`LUCIDOS_PREFIX`, `--port`/`LUCIDOS_PORT` (default 5252), `--bind`/`LUCIDOS_BIND`, `--tls-cert`/`LUCIDOS_TLS_CERT` + `--tls-key`/`LUCIDOS_TLS_KEY` (https opt-in), `--no-service`/`LUCIDOS_NO_SERVICE`, `--force`/`LUCIDOS_FORCE`, `--no-launch`/`LUCIDOS_NO_LAUNCH`, `--uninstall`/`--list`/`--all`/`--purge`, `LUCIDOS_HEALTH_TIMEOUT`. Provider creds (`OPENAI_API_KEY`/`VERTEX_PROJECT_ID`/`VERTEX_REGION`) are exported into the foreground gateway and **baked into the service env (mode 600)** when supplied. The env-as-flag contract means a dev shell that exports `LUCIDOS_TLS_CERT/KEY` — every engine-spawned subprocess does — silently configures TLS on a manual install run; the offline test suites `unset` them.

**Caveat (nothing published yet):** the CI workflow is artifact-only, so the default download **404s today** — the failure message points at `--dev` / `--from-tarball`. Offline-tested by `scripts/lib/install_test.sh` (download/extract path) and `scripts/lib/service_test.sh` (service.sh pure helpers + the foreground/degrade/register/uninstall wiring, all faked — no real launchd/systemd).
