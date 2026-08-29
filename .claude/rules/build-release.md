---
paths:
  - "scripts/build*.sh"
  - "scripts/check-staged-knowhow.sh"
  - "scripts/release*.sh"
  - "scripts/rebuild-mirror-history.sh"
  - "scripts/lib/build_*.sh"
  - "scripts/lib/stage_runtime*.sh"
  - "scripts/lib/resource_contract*.sh"
  - "scripts/lib/headless_tarball*.sh"
  - "scripts/lib/install*.sh"
  - "scripts/lib/service*.sh"
  - "scripts/lib/release_*.sh"
  - "scripts/lib/tauri_signing_key.sh"
  # Carries beforeBuildCommand, which is the build gate's only hook inside
  # `cargo tauri build`. Editing it is a build change, so this rule loads.
  - "crates/lucidos-app/tauri.conf.json"
  - "scripts/lib/updater_payload*.sh"
  - "scripts/lib/codesign.sh"
  - "scripts/lib/cargo_lock_holders_test.sh"
  - "install.sh"
  - "uninstall.sh"
  - "docker-entrypoint.sh"
  - "Dockerfile"
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

`scripts/lib/codesign.sh` is listed here **as well as** in `dev-runtime.md`, on
purpose. It is the dev signing identity's home, and since `build-dmg.sh` gained
its local-signing fallback it is also on the packaged build's critical path. A
path may appear in both lists, and both rules then load when it is touched,
which is the right outcome for a file that genuinely serves both.

`Makefile` was in this list until 2026-08-06 and is now claimed by
`dev-runtime.md` alone. It is overwhelmingly dev targets, and the double-listing
meant a one-line Makefile edit loaded both files: 172,149 chars of rules for a
change that touches neither pipeline. If a genuinely build-side target needs
saying, say it in `dev-runtime.md` and point across.

**The front door has its own rule.** `.claude/rules/front-door.md` owns the
`front-door`, `front-door-macos` and `front-door-parity` jobs, `front_door_*.sh`
and `release_rc_front_door*.sh`. It was a quarter of this file and loaded on
every edit to `install.sh`, `Dockerfile` or a build script, none of which can
change any of it.

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
| `install-smoke.yml` | push to `rc/**`, a `dmg_tag` dispatch (the RC draft gate, ADR 0036), `release: prereleased/released/published`, manual, weekly + daily cron | clean-machine `install.sh`, the notarized DMG, the tarball install, the live `lucidos.dev` front door (Linux + both macOS architectures) including its advertised **uninstall** paths, the RC front door's payloads, and **route parity between the two front doors** |
| `release-tarballs.yml` | `rc/**` push, `v*` tag push, manual | the per-triple headless tarball build, and attaching it to that tag's release (still a DRAFT at the time) |

**Nothing in there DEPLOYS, and that is a second rule, not a coincidence of the
first.** Publishing to a `lucidos.dev` origin runs on the maintainer's machine off
a workspace trigger, both halves of it: the landing page via `LucidosReleased` →
DMG-link bump → `SitePublishRequested` → publisher → `SitePublished`, and
`docs.lucidos.dev` via the "Publish lucidos.dev docs" trigger on
`LucidosReleased` (or an explicit `DocsPublishRequested`), which gates on
`mkdocs build --strict`, deploys `site/` to the `lucidos-docs` Pages project
through the engine's API proxy so the account credential never reaches argv or a
log line, verifies routes against that deployment's own preview URL, and emits
`DocsPublished` / `DocsPublishFailed`. The reason is the credential: a Cloudflare
token that can deploy Pages for this account also carries, in the form available,
`dns_records:edit` and `zone:edit` on the zone, which does not belong in a public
repo's CI for a deploy that need not happen there. `docs.yml` was that job. It
read a `CLOUDFLARE_API_TOKEN` secret the mirror never had, failed on **every**
release from 2026-07-11 through 07-31 while `docs.lucidos.dev` sat twenty days
stale, and was deleted rather than credentialed on 2026-07-31 (ADR 0031). No
workflow reads a Cloudflare credential now, and the `CLOUDFLARE_ACCOUNT_ID`
secret is referenced by none of them.

**Never add a workflow that compiles, lints, type-checks or tests the tree per
push/PR.** The repo is built and tested locally; the per-change gate is
`/harden` (`.claude/hooks/pre-push.sh` blocks a push without a FRESH marker, and
Apply runs it synchronously when the marker is missing). A CI gate could not
substitute even if you wanted one: Lucidos is **not PR-based** — Apply merges
the branch into `main` directly — so a `pull_request` trigger never fires and a
`push` trigger only reports *after* the change is already on main. A new
per-change check goes into `/harden` Phase 4.5's test-selection table
(`.claude/commands/harden.md`). This is the CLAUDE.md rule "We build locally:
GitHub Actions is RELEASE-ONLY"; the section here is its rationale and the
inventory it is measured against.

`install-smoke.yml`'s two crons are the only non-release *triggers*, and both are
deliberate — each verifies something **external** to the tree, which is why no
local run and no per-change gate could substitute:

- **weekly** (`0 4 * * 1`) re-runs the clean-machine install to catch drift in
  external toolchains (rustup, apt, Homebrew).
- **daily** (`0 6 * * *`) runs the `front-door` **and `front-door-macos`** jobs:
  the advertised `curl -fsSL https://lucidos.dev/install.sh | sh` against the
  **live deployed origin**, on a fresh `ubuntu:22.04` and on fresh macOS runners
  (both architectures). Every other job tests a tree or an
  artifact built from one; this tests what the site is serving right now, which
  regresses independently of any commit — on 2026-07-29 the Pages deploy
  published `install.sh` but not the `scripts/lib/*.sh` helpers a piped install
  sources, and because Pages soft-404s (landing-page HTML at status **200**)
  `curl -fsSL` succeeded and the installer sourced HTML as shell. The daily cron
  also runs **`front-door-parity`**, the only job that fetches BOTH origins.
  Those three jobs and their scripts are documented in
  `.claude/rules/front-door.md`, which loads when you touch them.

A schedule trigger fires the whole workflow, so **every job guards on
`github.event.schedule`** to claim exactly one cron. Adding a cron without that
guard silently multiplies the run frequency of every other job in the file.

Both are still delivery verification, not build gates.

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
`CleanBuildPassed` concern 4). The pin carries the same weight for **formatting**
since ADR 0030: rustfmt's output is a property of the toolchain too, and it is
what lets the fmt gate be stock defaults with no `rustfmt.toml`.

- **Bumping the pin is its own commit.** Change `channel`, run `make lint` plus
  the engine suite, and fix everything the new lint set surfaces *in that same
  change* (never let a stable bump red an unrelated branch). Since ADR 0030,
  `make lint` also runs `cargo fmt --all --check`, so a bump that moves rustfmt's
  output reds the gate too: run `make fmt` and carry the reformat in the same
  commit. Keep it separable from the channel change if the sweep is large.
- **Don't install a toolchain ahead of the pin in CI.** A `rustup … --default-toolchain stable`
  step that runs before checkout downloads a toolchain the pin then discards.
  `release-tarballs.yml`'s container step installs rustup with
  `--default-toolchain none` and lets the first post-checkout `cargo` call
  materialise the pinned one. Keep that shape. (The dev loop needs no such
  care: it builds locally, where rustup reads the pin from the tree.)
- **`rustup show active-toolchain`** in a CI log is the cheap way to see which
  toolchain a build actually used; `release-tarballs.yml` prints it.

### CI caches DEPENDENCIES only, and the release legs sit in their own namespace

`Swatinem/rust-cache`, in `release-tarballs.yml`'s matrix and in
`install-smoke.yml`'s `smoke` and `tarball-smoke` jobs. cargo is 94% of a
tarball job, and third-party dependencies are 12m25s of the Intel Mac's 32m16s.

**A release build may have one only because the cache is dependency-only.** The
action deletes the workspace members' artifacts from `target/` before saving, so
every release still compiles all of `crates/**` from source. What returns is
third-party code plus the cargo registry, and cargo integrity-checks the
registry against `Cargo.lock`. Cache a workspace crate here and that stops being
true.

Four rules, and each is asserted by `release_tarballs_gate_test.sh`:

- **Pin the action by commit, never by tag.** A release build must not follow a
  ref somebody else can move.
- **Key per leg.** The target triple and the container image go in the key. The
  action already folds in the rustc version, the job id and the lockfile hashes.
  Two legs must never alias, and a container bump must invalidate.
- **Only the `rc/**` arm writes** (`save-if`). The tag-push fallback and the
  backfill dispatch then read a cache written by a run of the same commit.
- **`prefix-key` fences release from smoke.** A poisoned cache in a test leg is
  a red run; in a release build it is a shipped binary. Nothing `install-smoke`
  writes may be readable by `release-tarballs`, and that is structural rather
  than a habit. For the same reason `tarball-smoke` does NOT share the Linux
  release leg's cache, though it runs the same script: one builds inside
  `ubuntu:22.04`, the other on the bare runner image.

The step goes **after** the first post-checkout cargo call. The action runs
`rustc -vV` to build its key, and the container legs install rustup with
`--default-toolchain none`, so nothing answers that until the pin is
materialised. `zstd` is in the container's apt list, or the cache falls back to
gzip.

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
- the frontend/binary builds, and `stage_runtime_assemble`, which takes **`<name>=<path>` pairs**, not ordered paths. Adding a resource used to mean a new argument in the middle of two call sites. Swapping two inputs of the same kind staged a tree that looked fine.

### The resource contract is checked against the launchers (ADR 0121)

**`scripts/lib/resource_contract.sh` is the ONE list**, and it is what both build scripts read `RESOURCE_NAMES`, `BUNDLED_EXECUTABLES` and the Tauri `bundle.resources` map out of. Do not restate any of them.

`resource_contract_check` asserts a **three-way set equality** against the two RUNTIME launchers, neither of which the build scripts own: `service_runtime_env_pairs` + `service_runtime_program` (`scripts/lib/service.sh`, the headless service), and the `*_RESOURCE_NAME` constants in `crates/lucidos-app/src/desktop.rs` (the packaged `.app`). Both name all seven. Drop one from the list and both other sources still carry it, so `--check` goes red on both vehicles.

That shape is the whole fix. `build-dmg.sh`'s old `check_resource_contract` compared `RESOURCE_NAMES` against a literal `resource_map_json()` **in the same file**, so editing both together passed. `build-headless.sh`'s `--check` was a `printf` and an `exit 0` and could not fail at all. Net effect: `system-knowhow` could be dropped from `Contents/Resources` **and** from `--emit-tarball` with every gate green, shipping an engine whose live per-turn reference docs are absent.

**A new resource is added in one place**, plus the launcher(s) that resolve it. `resource_contract_assert_staged` then re-reads the tree each vehicle actually wrote. A named call that simply omits a resource is therefore caught by the stage rather than by a user. Offline-tested by `scripts/lib/resource_contract_test.sh`, whose load-bearing half is the RED cases: it doctors each of the three sources in a scratch copy and asserts the check refuses, naming the resource. A green-only suite here would reproduce the bug it replaced.

`stage_runtime.sh` deliberately does NOT source the contract lib: `install.sh` fetches `stage_runtime.sh` over the network when piped, so a transitive dependency would have to be published beside it.

**The stage outlives the build, so it is guarded at the point of consumption.** `stage_runtime_assemble` `rm -rf`s its target before copying, so every sanctioned build path is correct by construction. What is not correct by construction is `build-dmg.sh`'s stage *between* runs: `crates/lucidos-app/bundle-resources/` is gitignored and survives, and a `cargo tauri build --config '<resource map>'` typed by hand skips step 4 and hands Tauri whatever that leftover holds. Found on 2026-08-07 carrying 12,732 characters of stale `system-knowhow` descriptions against 6,584 live, including a doc that had since been rewritten, with nothing anywhere saying so.

`stage_runtime_staged_knowhow_fresh` byte-diffs the staged copy against the live tree, and `scripts/check-staged-knowhow.sh` runs it from `tauri.conf.json`'s `beforeBuildCommand`, the one hook inside **every** `cargo tauri build` regardless of caller. Ordering is safe: `build-dmg.sh` stages at step 4 and builds at step 5, so the guard always sees a stage it just wrote. **Absent is clean, and must stay so**: a developer who has never run `build-dmg.sh` has no `bundle-resources/` at all and their build must not go red. Only `system-knowhow/` is checked, because it is the one staged resource that is a git-tracked SOURCE tree with an exact live counterpart; the binaries, `postgres/`, `frontend/` and `sdk/` are build outputs where "fresh" is undefined without rebuilding, and a timestamp heuristic there would fail open. A new staged resource of the source-tree kind should join the check.

Pure helpers are offline-tested by `scripts/lib/stage_runtime_test.sh`. The `lucidos` CLI is **load-bearing**, not a convenience: the engine resolves it as a sibling of `lucidos-engine` (`find_lucidos_cli_dir`), or by absolute path via `LUCIDOS_CLI_BIN` when the launcher stamps it (`desktop.rs::spawn_gateway` / `service_runtime_env_pairs`), to launch the Claude Code permission-prompt MCP server (`lucidos mcp-permission-server`). A bundle omitting it breaks every coding-agent thread on its first tool call — the engine now fails the CC spawn fast with a descriptive error rather than starting a doomed session (`resolve_lucidos_binary` in `crates/lucidos-engine/src/runtime/claude_code.rs`).

**Headless tarball — macOS signed (`build-dmg.sh --emit-tarball`).** In addition to the `.app`/`.dmg`, emits a per-platform `lucidos-<version>-<target-triple>.tar.gz` (the 7 `RESOURCE_NAMES`) plus a `shasum -a 256 -c`-compatible `.sha256` sidecar — the Docker-free, compile-free download artifact `install.sh` lays down (step 1 of `docs/plans/2026-06-30-installer-step1-headless-tarball.md`). Sourced from the SIGNED `.app` `Contents/Resources/` (not `bundle-resources/`, whose copies are never signed), so the Mach-O files keep their Developer ID signatures. Opt-in, applies to any build mode (no-op under `--check` / a build-less `--release-attach`); default behavior unchanged when absent.

**Headless tarball — Linux + macOS unsigned (`build-headless.sh`).** The Tauri-free build path (step 2 of `docs/plans/2026-06-30-installer-step2-linux-tarball.md`). Runs the shared staging for the **host** triple — no `cargo tauri build`, no `.app`, no DMG, no codesigning — then reuses `headless_tarball_emit` for the same `lucidos-<version>-<triple>.tar.gz` + `.sha256`. On **Linux** this is THE release build path; on **macOS** it produces an UNSIGNED tarball (use `build-dmg.sh --emit-tarball` for the signed one). It compiles natively, so `--triple` must equal the host — cross-arch artifacts come from the CI matrix's per-arch runners. Flags: `--triple`, `--out-dir` (default `.lucidos/release-staging/<version>/`), `--version` (default RELEASE → tauri.conf.json → 0.0.0), `--check`. Offline-tested by `scripts/lib/build_headless_test.sh`.

**Linux tarballs via CI (`.github/workflows/release-tarballs.yml`).** A `workflow_dispatch` + `rc/**`-branch-`push` + `v*`-tag-`push` matrix over the four target triples (`x86_64-unknown-linux-gnu` is the must-work entry; macOS x86_64 + Linux aarch64 are best-effort; `fail-fast: false`). Each entry runs `build-headless.sh` on a **native** runner, with the Linux entries INSIDE an `ubuntu:22.04` container. Every entry uploads the tarball + `.sha256` as a **workflow artifact**, and then ATTACHES them to the release carrying the pushed tag. It never creates or tags a Release.

That container is the **glibc 2.35 floor**. A binary built on the raw 24.04 runner image refuses to start on Ubuntu 22.04, Debian 12 or RHEL 9, with `GLIBC_2.3x not found`. The same-machine tarball-smoke cannot see that. An "Assert portability floor" step guards it, failing the build if any staged binary references a `GLIBC`/`GLIBCXX`/`CXXABI` symbol version above that floor.

**The TAG PUSH is what attaches, and the release it attaches to is a DRAFT (2026-08-04).** There used to be a third trigger, `release: types: [published]`, and it produced the whole problem: the Release went public with only the DMG, that publish event started a SECOND full build of the same four tarballs (v0.21.0 ran 30926097107 on the tag push and 30926100020 on the release event, two seconds apart), and the tarballs landed 11 to 35 minutes later. Inside that window the advertised `curl … | sh` genuinely 404s. Now `release-to-lucidos.sh` creates the Release as a draft, this run attaches to it, and the publish waits for all four (ADR 0042, and Phase B under "the release candidate IS the published artifact" below). The `release:` trigger is gone: it cannot start the build any more (a draft fires no webhook) and keeping it would mean publishing the draft kicked off a duplicate 35-minute build that re-attached over files already there. A manual `workflow_dispatch` with `attach_to_release=true` (+ `attach_tag`, or a tag ref) is the backfill arm, and it is what `release_draft_wait_for_assets` dispatches when a run finishes having attached nothing.

**The rc BRANCH builds the same tarballs an hour earlier, and Phase B attaches them.** The tag push is the last thing a release does. Waiting for that build put a whole uncached Intel-Mac cargo compile on Phase B's serial path, 34m23s of its 35m32s on v0.29.0. The tree already exists at Phase A: the wrapper pushes the deterministic stripped orphan commit to `rc/<version>`, and Phase B force-pushes **that same object** to `main`. So an `rc/**` push builds precisely what ships, while Apple notarizes. That arm stops at the workflow artifact, because the attach step's `if:` admits only a `v*` tag push or an opt-in dispatch.

`release_draft_wait_then_publish` is where the two meet, so both Phase B entry points get it from one function. It calls `release_draft_attach_from_rc_run` first and then waits exactly as before. **The pin is the commit.** `release_tarball_rc_run_id` returns a run only when its `head_sha` equals the object being published and its head branch begins `rc/`. It re-asserts the `head_sha` the caller already filtered on, so a filter that silently stops applying cannot promote an unrelated build. Then it proves the bytes: every expected name resolves to exactly one file, and every tarball verifies against the sidecar its own build wrote.

**The updater trio has no route from CI onto a release.** The uploaded set is derived from `release_draft_expected_assets`, which names only the eight tarball assets. Anything else in the downloaded artifact is reported and left where it is.

**Every refusal is a fallback, never a failed release.** No rc run, a run still going, a failed one, expired artifacts, a sha256 that does not verify, a missing platform: each says why, and the tag-triggered wait takes over unchanged. Degrade to slow, never to wrong.

Two consequences hold that together. `release_tarball_run_verdict` **ignores `rc/`-branch runs**, because both runs carry the same head commit. A finished rc run beside an unregistered tag run would otherwise read as success-with-nothing-attached, and dispatch a needless backfill.

And the CI attach step **never deletes an asset from a PUBLISHED release**, only from a draft. The fallback run now reaches a live release half an hour late, and the gap between DELETE and POST is a window where `curl … | sh` 404s. Retention splits the same way: 7 days on the rc arm, which Phase B downloads, against the tag arm's 1-day inspection copy.

**Finding the release needs a LISTING, not the tag endpoint.** `GET /repos/{owner}/{repo}/releases/tags/{tag}` does not resolve a DRAFT (a draft has no tag ref), so the step pages `GET /releases` and matches `tag_name`, which also means `permissions: contents: write` is load-bearing for READING: GitHub lists drafts only to a caller with push access. Two refusals keep the rc gate out of it: a resolved tag beginning `rc-`, and a release whose `prerelease` is true. Upload goes through the raw GitHub REST API with `curl`, NOT `gh release upload`: the Linux matrix entries run inside the `ubuntu:22.04` container, which has no `gh`. Offline-tested by `scripts/lib/release_tarballs_gate_test.sh`, which also asserts the matrix triples equal `release_draft_triples` so a platform cannot be added to one and silently missed by the other.

**The macOS headless tarballs on a Release are the UNSIGNED CI ones**, including the one a Mac's `curl … | sh` downloads. `build-dmg.sh`'s upload attaches exactly the DMG + `Lucidos.app.tar.gz` + `.sig` + `latest.json`, and `release.sh` never passes `--emit-tarball`, so the local SIGNED headless tarball exists as a capability but is never attached to anything.

**The reason is a principle, not the current wiring (ADR 0034).** The Developer ID identity lives only on the release machine and cannot be handed to CI, the same boundary ADR 0031 drew for the Cloudflare deploy credential, so **any artifact CI builds is ad-hoc by construction**. Its consequence is the second half of the rule and must be stated with it: the headless install path has a code identity that changes on every build (an ad-hoc Mach-O's designated requirement is a bare `cdhash`, which moves with every compile). This replaces the old justification, *"there is no signed macOS tarball for CI to clobber"*, which described that day's wiring rather than a rule and read as though wiring `--emit-tarball` in would be a straightforward improvement. It would not be: the signed tarball is built on the release machine for the HOST triple only, while the four published ones come from a CI matrix over four triples on native runners, so signing them means either building all four somewhere with no Linux runner and no Intel Mac, or moving the identity into CI.

The cost of that consequence is measured in ADR 0034 rather than asserted, and it is near zero **by default**: everything the headless install touches (`~/.lucidos/runtime`, the per-instance dir, gateway-provisioned workspaces, which resolve RELATIVE to app-data, loopback and outbound network) is outside every TCC-gated location, and the engine / gateway / CLI declare no Apple-framework dependencies at all, so there is no camera, microphone, screen-recording, accessibility, contacts, calendar, photos, location or Apple Events path to gate. It is NOT zero where a user points a workspace or a coding-agent repository at Documents / Desktop / Downloads: there the per-build identity discards the grant at every update. Read ADR 0034 before changing this, including its list of what would reopen the decision.

Two other things keep the unsigned front door safe, and both predate the principle: a `curl`-fetched file carries no `com.apple.quarantine` xattr, so Gatekeeper never assesses the runtime (the same reasoning ADR 0027 relies on to defer notarization), and `install.sh`'s `verify_runtime_executes` runs the gateway once at install time so any refusal is loud and immediate. Asset timings on v0.17.0 (DMG trio 04:08, all eight headless assets 04:19–04:51) are how to confirm which path produced which asset.

**After F1 the front door is the ONLY unstable code identity of the three.** The same engine gets a different identity per install path: the DMG's is Developer ID with a certificate-anchored DR (always was stable), the updater payload's is now the same because the payload is repacked from the signed app (stable since F1; ad-hoc with a per-build cdhash on every release through v0.19.0), and the headless tarball's is ad-hoc `lucidos_engine-<crate metadata hash>` (still unstable). That asymmetry used to be background and is now the exception, which is why it is recorded rather than left as a consequence.

Packaging lives in `scripts/lib/headless_tarball.sh` (offline-tested by `headless_tarball_test.sh`); it copies with `ditto` on macOS (preserves embedded Mach-O signatures) and `cp -a` elsewhere (Linux runners have no `ditto`).

### The release candidate IS the published artifact (rc-first, ADR 0024)

A release is **one stripped tree, built once, tested, then promoted** — never a
tree that is validated and a second tree that ships.

**One strip implementation: `scripts/lib/release_tree.sh`.** It owns
`RELEASE_TREE_EXCLUDE_PATHS` (internal-only paths: `docs/plans/`, `release.sh`,
`release-to-lucidos.sh`, the one-time `rebuild-mirror-history.sh`, the
`release_signing` / `release_events` / `release_main_sync` /
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

0. reads the mirror's `main` and its `v*` tag count, and **refuses the release**
   unless `rev-list --count main` equals that tag count (see "the mirror's
   `main` is a linear release history" below). This runs before the tree is
   built, because it invalidates the whole release;
1. builds the stripped tree from the release commit, **scans it**, commits it
   **onto the mirror's `main`** (ADR 0039), and force-pushes it to
   `refs/heads/rc/<version>`, *before* the
   DMG build, so `install-smoke.yml`'s slow Ubuntu/macOS source-install legs run
   concurrently with the build and the private-data guard refuses a leaking tree
   before 40 minutes of build time, not after;
2. records `RC_COMMIT` **and `RC_PARENT`** in `verify-build-<version>.env`
   (alongside `SOURCE_COMMIT`, `PR_NUMBER`, `PR_TITLE`), pinned locally at
   `refs/release-candidates/<version>` so the object can't be GC'd between phases;
3. after staging, deletes + recreates the `rc-<version>` **draft release** at that
   branch with the staged DMG + updater `.sig`, then DISPATCHES the `dmg-verify`
   leg at it (`-f dmg_tag=rc-<version>`). A **draft** so the gate artifact is
   never listed on the public releases page (as a prerelease it sat above the
   current GA for the whole Phase A to Phase B window, days on a deferred
   notarization), and an **explicit dispatch** because GitHub fires no workflow
   event whatsoever for a draft release. Do not try to bring back an
   event-driven trigger by adding `created` to the workflow's `release: types:`;
   it does not fire for drafts and would double-run the job for a non-draft rc.
   See ADR 0036, which also covers why `dmg-verify` must keep
   `permissions: contents: write` (a draft is invisible to a `contents: read`
   token).

A **notarize resume** reaches both steps too. **`release.sh --push-rc <version>`**
re-arms the gate from the recorded release commit with no rebuild (a failed
push, a replaced rc, or a state file predating this flow). Both paths are
idempotent: an unchanged candidate is a no-op push, and an existing remote rc
whose *tree* **and parent** both match is **adopted** rather than replaced, so a
green gate is not thrown away. Parent equality is the half that matters since
ADR 0039: "parentless" stopped being the safety property, and an object with the
same tree but a different parent would put unscanned ancestry on the mirror.

**Phase B (`release.sh --publish-verified`) promotes; it never rebuilds.**
`release_promote_preflight` refuses, before the confirm prompt and before
anything public, when: no `RC_COMMIT` was recorded, `rc/<version>` is gone from
the mirror, the mirror's rc **moved** (someone re-pushed ⇒ the gate result is
stale), **no `RC_PARENT` was recorded** (a state file predating ADR 0039, whose
candidate would flatten the history), **the mirror's `main` moved** off
`RC_PARENT` (a release landed since, so the candidate's parent is stale),
`manifest.source_commit` ≠ the worktree HEAD, or any staged artifact's
sha256 drifted. It also **refuses the wrong arity**: the parent pair was
appended to a five-argument signature, and a caller left at five would read two
empty strings and skip the parent half in silence. Then
`release-to-lucidos.sh --promote-rc <sha> --parent <sha>` re-asserts the
unmoved rc, re-scans that commit's tree (the deterministic floor at the
irreversible push), and pushes **that same object** to the mirror's `main`
under a lease + tags it `v<version>` **by SHA**, creates the GA Release as a
**DRAFT**, attaches the staged artifacts, WAITS for `release-tarballs.yml` to
attach the four per-platform tarballs, publishes the draft and only then emits
`LucidosReleased`; the rc branch + rc draft release are deleted afterwards. It
attaches the rc build's tarballs first, so that wait normally returns on its
first poll. Where it falls back, the wait is the old 25 to 45 minutes. Either
way it is resumable: nothing is public while it runs, so an interrupted one
costs `release.sh --publish-draft <version>` and no rebuild (ADR 0042).

The legacy one-shot (`release.sh <version>`, no phase flag) still builds its own
tree from HEAD through the same lib and has no rc gate; it resolves its parent
from the mirror itself, at the point of publishing, and is held to **both** ADR
0039 rules there (the shared `release_mirror_history_check`, and adopting a
version the mirror already publishes rather than rebuilding it, which is what
keeps a retry after a partial publish from adding a second commit for one tag).
Offline-tested by
`scripts/lib/release_tree_test.sh` (strip coverage, self-exclusion, guard
fail-closed on both arms, commit determinism with and without a parent, the
full ancestry-guard matrix, both pure push/history comparators including both
of the count check's subtrahends, the recorded-exception verifier against a
stand-in mirror in all three of its stale directions (tagged, off-history,
abbreviated), every preflight
refusal, an end-to-end rehearsal against a throwaway bare repo, and the wiring
that keeps the promotion a promotion).

#### The mirror's `main` is a linear release history (ADR 0039)

Release N's published commit carries release N-1's as its **single** parent, so
the mirror shows one commit per release and a release is ancestry-preserving
rather than a history-replacing force-push. Until 2026-08-04 each release was a
PARENTLESS commit force-pushed over the last, which is why the mirror showed a
**one-commit history with 36 unrelated tags** and why every release broke every
existing clone.

What makes a parent safe is not that parents are harmless. A push sends every
reachable object while `release_tree_scan` only inspects the **tip tree**, so
ancestry genuinely could reach the mirror unscanned. It is that the parent is
**required to be the object the mirror's own `main` already holds**, so the push
adds nothing the public does not already have. `release_tree_is_orphan` became
`release_tree_ancestry_is_published <repo> <commit> <expected-parent>`, which is
the precise version of the property the blunt rule stood in for: at most one
parent (a merge is refused, the history is strictly linear), that parent exactly
the mirror's `main`, and no parent accepted only when the mirror has no `main`.
Each refusal names which of the three it is, because the next move differs.

Four things hold it together, all in `scripts/lib/release_tree.sh`:

- **`release_mirror_branch`** is the ONE definition of the published branch name.
  `release-to-lucidos.sh` assigns `BRANCH` from it after sourcing the lib rather
  than carrying a literal, because the same name is also what
  `release_mirror_main_sha` reads and what the lease is composed against.
- **The parent is decided once**, in Phase A, and recorded as `RC_PARENT`. It is
  part of the candidate's identity, so a rebuild onto a moved mirror is
  deliberately a DIFFERENT object: the CI verdict on the old one does not carry
  over to a different history.
- **`release_main_push_decision <mirror-main> <candidate> <parent>`** picks one
  of four words. `published` (main already IS the candidate) is the load-bearing
  one: it is what keeps a retried publish idempotent, because the first
  successful push moves `main` off the parent and a lease alone would then make
  the second run refuse itself. `lease` pushes with
  `--force-with-lease=refs/heads/main:<parent>`, which puts the precondition on
  the SERVER at update time where the confirmation prompt cannot race it.
  `create` is the empty-mirror bootstrap; `refuse` means a release landed since.
- **`release_mirror_history_is_complete <commits> <tags> <in-flight> <recorded>`**
  is the precondition,
  and it is permanent rather than a one-time step. It refuses in BOTH
  directions: fewer commits than release tags means the history is missing
  releases (the pre-repair state: 1 against 36, with the refusal naming
  `scripts/rebuild-mirror-history.sh`), more means something reached `main` that
  no release published and that nothing accounts for. Once true it
  self-maintains, since every release adds exactly one commit and one tag.
  The count is necessary but **not sufficient**, so
  `release_mirror_tags_are_on_main` runs after it: a tag rewritten onto an
  unrelated commit, or one deleted while another is added, leaves the totals
  equal while the history stops accounting for the releases it advertises. That
  second half costs no extra fetch and creates no local ref, which is what makes
  it affordable on the release path. Counting `main` already fetched it, and a
  fetch brings everything REACHABLE, so a tag whose object is still absent is by
  that fact not on `main`. Fetching the tags themselves is deliberately avoided:
  `git fetch --tags` would plant the mirror's stripped commits under local `v*`
  names, which is exactly the ADR 0029 regression. The count runs first so the
  pre-repair state still gets its actionable "run the repair" message rather
  than a list of 36 stray tags.
  **Every publish path calls it**, through the one shared
  `release_mirror_history_check`: Phase A before it builds a candidate, **Phase
  B again in its promotion guard**, and the legacy one-shot before it builds its
  own commit. The one-shot is the path with no rc gate, so it is exactly where a
  missing check goes unnoticed. Phase B repeats Phase A's because the parent
  check only proves `main` has not MOVED and says nothing about the tags, while
  Phase A's verdict can be **days** old: a deferred release holds that window
  open for as long as Apple takes, so a `v*` tag added, deleted or rewritten in
  between would otherwise publish onto a history that no longer balances. Its
  in-flight count is 1 exactly when the mirror already names the candidate, i.e.
  a re-run after `main` was pushed and the tag push failed.

**The one-shot also has to adopt, not rebuild.** Its re-runnability used to come
entirely from the commit being parentless and deterministic (a retry rebuilt the
identical object and the push was a no-op). A parent removes that, and the
failure is silent and permanent: a retry after a partial publish reads the
commit it just published as its own parent and builds a SECOND commit for the
same version, which breaks the completeness check for every release after it.
So a version the mirror already publishes **with an identical tree is adopted**
(`release_mirror_tag_sha`, the same idiom Phase A uses for a matching rc), and a
tag that exists with a **different** tree is refused: re-releasing one version
with different content leaves the tag, the Release page and every download URL
disagreeing, and the answer is a version bump.

**The window between the two pushes is why the check takes an in-flight count,
and why adoption is resolved BEFORE it.** Publishing is `main` first, then the
tag; if the tag push fails, `main` legitimately carries one commit no tag names.
Ordered the other way round, the retry whose only remaining job is to push that
tag hits "more commits than releases" and is refused for the state it exists to
repair, with no in-workflow escape and every later release refused too. That
deadlock is worse than the stray-hand-push it was guarding against. So the check
takes the count as a **required** argument (0 defaults to the deadlock; 1 would
blanket-excuse a stray commit), Phase A always passes 0 because it pushes to
`rc/<version>` and never to `main`, and the one-shot passes 1 only once it has
proven `main` is its own untagged commit. Tree equality is the proof, and it is
sound because a release commit always bumps `RELEASE`, `CHANGELOG.md` and
`install.sh`, so two consecutive releases can never share a published tree.

**A run that NEVER comes back is the second exception, and it is recorded rather
than assumed (2026-08-11).** The in-flight count covers the two-push window only
for the run that returns to finish the tag; nothing covered one that died in
between and was superseded. One did: the first v0.26.3 Phase A pushed its
stripped commit to `main` and died in `notarize` (notarytool `abortedUpload` /
`connectTimeout`), a later run parented a fresh candidate on the leftover as
designed and published it, and the mirror was left at 49 commits against 48
tags, refusing every release. The leftover cannot be dropped, because it is the
PARENT of the published v0.26.3 commit and removing it changes that commit's SHA
and breaks every clone, so the COUNT learns about it instead:
`RELEASE_MIRROR_HISTORY_EXCEPTIONS` in `release_tree.sh` records the SHA, the
date and the reason, one line per commit. **An entry is never trusted.**
`release_mirror_history_exception_count` re-asks the live mirror both halves on
every run (still an ancestor of `main`, named by no `v*` tag) and only a verified
entry is subtracted, because a bare recorded number would be an unconditional -1
that silently absorbs a DIFFERENT stray commit once the recorded one stops being
what its entry describes. **A stale entry refuses**, naming it, rather than being
skipped: an entry that stops verifying means either the commit was tagged after
all (the list is hiding a real release) or the mirror was rewritten underneath a
published one, and quietly dropping it would downgrade a corrupted mirror to a
merely stricter count.

**The SHA must be the full 40 hex, and that is correctness rather than tidiness.**
git resolves short object names, so an abbreviation satisfies the ancestry half
on its own, while the tag half compares against 40-char `ls-remote` output and
can never match. An abbreviated entry would therefore verify forever with its
"still untagged" half never actually running, and would go on subtracting the
commit even after a tag landed on it. The shape is refused up front rather than
expanded, since expanding it here would make the recorded list mean whatever the
local object store happens to resolve it to.

The two subtrahends stay independent and compose: in-flight is a commit whose tag
is still coming, an exception is one whose tag is never coming. The list lives in
`release_tree.sh` because that file is already withheld from the mirror, so it
introduces no new exclude entry and no new leak surface, and nothing in the
pipeline writes to it: adding an entry is a human decision, and the list is meant
to stay at one, because a second means Phase A dropped another commit on `main`
and that is a bug to fix at the source. Registered in `docs/temporary-measures.md`
(a mirror rebuild would re-derive a chain with no untagged commits, which is the
removal condition).

**The one-time repair still has to run once, by a human.** Chaining onto a
one-commit `main` only ever yields a two-commit `main`, so
`./scripts/rebuild-mirror-history.sh --push` rebuilds the 36 published releases
as a chain first (atomic, leased per ref, with a rollback bundle and a typed
confirmation). The precondition above is what makes that ordering enforced
rather than remembered. The script is in `RELEASE_TREE_EXCLUDE_PATHS`: it
sources `release_tree.sh`, which is withheld, so the copies published at v0.20.0
and v0.20.1 can do nothing but print their own refusal. Delete it once the
rebuilt history is on the mirror (`docs/temporary-measures.md`).

### The source side: main gets the bump, and the tag names it (ADR 0029)

The same tag name means a **different object per remote**, deliberately: the
mirror's `v<version>` names the **stripped published release commit** (the
Release and every download URL resolve through it), while the **local** and
**`origin`** tag names
the release commit on **`main`**. So the mirror tag is pushed **by SHA**
(`push --force <remote> <commit>:refs/tags/<tag>`), touching no local ref —
creating it locally first is what left 26 of 27 `v*` tags outside main's history,
made `git describe --tags main` report `v0.9.6-4946-gfb4b344cf`, and rendered
every `PREV_TAG` guard in `release.sh` vacuous.

`scripts/lib/release_main_sync.sh` owns the source side, wired in through ONE
`settle_source_side` entry point that both Phase B and the one-shot call:

- **The bump is LANDED on main, not attempted.** Fast-forward when possible;
  **cherry-pick** the single release commit when `main` moved during the build;
  **hard-fail** (after `cherry-pick --abort`, so nothing is left wedged) on a
  conflict. Only operator state — not on `main`, or dirty — still skips. The old
  `advance_local_main` warned-and-continued instead, which is how **v0.17.0**
  published while `main` never learned its own version and the site kept serving
  the previous DMG (the site publisher reads the local checkout's `RELEASE`).
- **Skips and failures are reprinted at the END of the run**, in a `STILL OWED`
  block with the exact recovery commands. A warning buried mid-build-log is a
  warning nobody reads — that is the actual v0.17.0 failure.
- **`origin` is pushed at publish time and NEVER forced** (`main` + the tag). A
  failure there is a loud post-release warning with a retry command, never an
  unwind: the release is already public, and a non-fast-forward `origin/main`
  means the maintainer has work this checkout has not fetched.
- **The tag is idempotent.** An already-correct tag is left byte-identical (so
  the `origin` push stays a no-op rather than needing a force); one left on the
  orphan by an older release is force-moved onto the main-line commit.

**The `PREV_TAG` drift guards are honest again — with two adjustments.** They now
gate on real ancestry (`release_tag_is_ancestor`), and when `PREV_TAG` is a
legacy orphan they *degrade to advisory* with a one-line note rather than
exploding — the FIRST release after this change still sees one. And because the
deleted-files gate finally diffs two full **internal** trees, it filters out
paths withheld from the public tree via `release_tree_path_is_excluded`;
otherwise ordinary `docs/plans/**` churn would start refusing releases over files
that can never reach a user. `PREV_TAG` resolution stays a **semver sort**, not
`git describe` — describe answers "nearest *reachable* tag" and so silently picks
an older one exactly when the newest is an orphan.

Offline-tested by `scripts/lib/release_main_sync_test.sh`: every landing state
against throwaway repos, the conflict-abort, the by-SHA mirror push into a local
bare repo (asserting no local tag appears), the unforced `origin` push and its
non-fast-forward rejection, behaviour under `release.sh`'s `-Eeuo pipefail` +
exiting ERR trap, and the wiring in both release scripts.

### Notarization is resumable — never a foreground `--wait`

Apple's notary service routinely outlives the process waiting on it, and the orchestration layer caps background tasks at **3600 s**, so a slow notarization can never be held in a foreground wait. `build-dmg.sh` therefore submits with **`--no-wait`**, **persists a resume handle before any waiting** (`<repo-root>/.lucidos/release-state/notarize-<version>.json`, written atomically by `scripts/lib/release_notarize.sh`), then polls `notarytool info`.

**One handle carries BOTH notary submissions.** A release notarizes the `.app` first and the DMG second (ADR 0033), so the handle records a `stage` field (`app` | `dmg`) naming which is outstanding, plus `artifact_path` / `artifact_sha256` for whichever file was handed to notarytool (the app zip, or the DMG), `app_path` / `app_cdhash` at the app stage, `version`, `source_commit`, `submitted_at`, and the three **pairing** fields `updater_tarball_path` / `updater_tarball_sha256` / `updater_sig_sha256`. Every key is always written, empty when it does not apply, so a MISSING key means exactly one thing: a handle from before this shape, which `release_notarize_resumable` refuses by name rather than reading as "no pairing recorded, so nothing can mismatch". `dmg_path` / `dmg_sha256` were renamed to `artifact_*` when the app stage made the old names a lie at one of the two stages. Losing the waiter costs a poll, not a rebuild. Before this, a killed wait threw away a full cargo release build, ~134 inside-out codesigns, and a signed DMG, because the staple, the staging dir, the `manifest.json`, and the only copy of the submission id all died with the process.

- **Resume:** `release.sh --resume-notarize <version>`, or `build-dmg.sh --resume-notarize` directly. It branches on the handle's `stage`: an `app` stage polls, staples the bundle, and then runs on into the DMG half through the same `run_dmg_notarize_stage` a fresh build uses, so a resumed release and a fresh one build the image through identical code. Re-running `release.sh --verify-build <version>` auto-promotes to a resume when a handle exists (and then no longer requires `-c`, since that changelog is already committed). The resume reuses the EXISTING Phase A worktree and writes the `verify-build-<version>.env` state the killed run never reached, so `--publish-verified` works afterwards.
- **Adopt:** `build-dmg.sh --adopt-submission <uuid>` records an in-flight DMG submission whose id was never persisted against the on-disk DMG, then resumes. `--adopt-app-submission <uuid>` is its sibling for the `.app` half (it needs the `<app>.notarize.zip` still on disk, since that is the only record of what Apple scanned). Only one may be given: one submission is outstanding at a time, and adopting both would write one handle over the other and silently discard a live submission.
- **The resume gate is strict** (`release_notarize_resumable`): the handle must carry every field of the current shape, name a known stage, its submitted artifact must still hash to what was submitted, the **paired updater payload and `.sig` must still hash to what was recorded**, AND the tree must still be on the recorded `source_commit`. The source-commit half is load-bearing: the resuming run stamps `manifest.source_commit` from its own HEAD, so resuming on a moved tree would make the staging manifest claim a commit the DMG was never built from, and `--publish-verified`'s identity guard would pass on a lie. A build-grade run that finds a NON-resumable handle says why and rebuilds; an explicit `--resume-notarize` fails loud.
- **Terminal cases:** `Accepted` → staple (idempotent, via `stapler validate` fallback) + stage; `Invalid`/`Rejected` → print the notary log and refuse to stage; an id Apple doesn't recognise → say so and require a fresh submit (never silently re-submit). The handle is dropped once staging succeeds, so a later run can't resume a finished release.
- **Credentials are unchanged and load-bearing.** One `notarytool_run` wrapper resolves them for `submit`/`info`/`log` alike: App Store Connect API key first (`-i` only when `APPLE_API_ISSUER_ID` is set — required for Team keys, must be omitted for Individual ones), else `APPLE_PASSWORD` piped on **stdin**. Never `--password` in argv (world-readable via `ps`), never `store-credentials` (headless ⇒ "User interaction is not allowed"). `build_dmg_test.sh` asserts all of this against the notarytool call sites.

Offline-tested by `scripts/lib/release_notarize_test.sh` (the pure handle: round-trip, checksum/commit/missing-DMG refusals, UUID shape, notarytool JSON parsing) and the resume section of `build_dmg_test.sh` (flag parsing, the end-to-end gate, and the `release.sh` phase plumbing). Knobs: `NOTARIZE_POLL_INTERVAL` (30 s), `NOTARIZE_POLL_TIMEOUT` (7200 s — bounds the process, not the handle), `NOTARIZE_POLL_MAX_FAILURES` (5).

### Preparing a release unattended: preflight, deadline, abandon

Three flags on `release.sh` let a nightly take a release from "should we?" to
"built, gated, waiting for a human" and stop dead there. **None of them
publishes, pushes to `main`, creates a `v*` tag or publishes a Release**, and
that is the property to preserve when touching any of them.

- **`--prep-preflight <version|auto>`** is a READ-ONLY verdict machine
  (`scripts/lib/release_preflight.sh`, withheld from the mirror): one JSON object
  on stdout, progress on stderr, and an exit code that is the contract (**0** GO,
  **10** SKIP, **1** ERROR). Zero side effects, so it is safe mid-flight. Its
  base selection follows the release knowhow and must never become `git
  describe`, which answers "nearest *reachable* tag" and so picks an older one
  exactly when the newest is an orphan. Two asymmetries are deliberate: an
  unreadable mirror fails CLOSED to skip (stacking a second release on an
  unfinished one is unrecoverable by script), while an unreachable CI-weather
  signal is `"unknown"` and never a skip on its own. It is also stricter than
  `release.sh`'s own deletion gate: no intent-keyword exemption, because an
  unattended run may never decide for itself that a deletion was intentional.
- **`--notarize-deadline <spec>`** bounds the wait for Apple on `--verify-build`
  / `--resume-notarize`. On expiry `notarize_poll` returns instead of dying, the
  run exits 0, the `notarize` step is closed as **Succeeded** with the verdict
  recorded as outstanding, and **nothing is staged**. That last part is the
  safety property: with no staging dir there is no manifest `--publish-verified`
  could promote. With a deadline set it REPLACES `NOTARIZE_POLL_TIMEOUT` rather
  than sitting beside it (two bounds means the shorter wins, which would defeat
  a deadline further out than the 7200s default). Do not confuse it with
  `--defer-notarization`, its opposite in intent: deferring stages an unstapled
  DMG so the release can publish behind a banner. They are refused together.
- **`--abandon <version>`** removes a Phase A's whole footprint in one
  idempotent command with a per-item table, and REFUSES once `v<version>` is
  published or the mirror's `main` already names it. That is a yank, a different
  and far more dangerous operation. It never touches the workspace changelog
  artifact; it reports where it is.

Both Phase-A entry points invoke the build through **one** helper,
`run_release_build_dmg`, which owns the interpretation of build-dmg.sh's exit
status. Keep it that way: a second call site would be a second place deciding
whether a deadline pause is a failure, and its `|| rc=$?` sits INSIDE the
subshell because `set -E` puts release.sh's exiting ERR trap in there.

Offline-tested by `release_preflight_test.sh`, `release_deadline_test.sh` and
`release_abandon_test.sh` (throwaway git repos, a fake `gh`, a `file://` status
URL, and the build-dmg.sh functions pulled in by the same awk extraction the
other suites use).

### The release does not wait on Apple — deferred DMG (ADR 0027)

Resumability stopped a slow verdict costing a *rebuild*; it did not stop it blocking the *release*. **`--defer-notarization` does, for the DMG's verdict.** Since ADR 0033 a release makes TWO notary submissions in Apple's order, the `.app` and then the DMG, and only the second is deferrable: the DMG is built FROM the stapled app, so there is nothing to stage until the app's verdict lands. A deferred release therefore waits once (1 to 20 hours, 8h06m on v0.16.0) rather than not at all. That is the cost of F5 and it is recorded in ADR 0033, not hidden here. Notarization still gates exactly one *shipped* artifact, the `.dmg` a browser downloads. The headless tarball and the updater trio (`.app.tar.gz` + `.sig` + `latest.json`) are never quarantined, so Gatekeeper never assesses them: existing users and `curl … | sh` installs are wholly unaffected.

- **Phase A**: `release.sh --verify-build --defer-notarization <version>` submits, persists the handle, and stages the **unstapled** DMG with `notarized: false` in the manifest. With an already in-flight submission it stages **without polling**, which is the rescue path for a Phase A stuck on a slow verdict. It does **not** create the `rc-<ver>` draft release (`arm_dmg_gate_if_notarized`): `dmg-verify` asserts a stapled ticket, so arming a gate that must fail says nothing.
- **Phase B** — `--publish-verified` publishes with the *notarization-pending banner* on the Release body and **keeps** the worktree, staging, state file, notarize handle and submitted-bytes pin. Those are the attach step's only inputs; deleting them would strand a published DMG unstaplable.
- **Finish** — `release.sh --attach-notarized <version>` polls, staples, re-stages, `--clobber`s the asset in place, rewrites the body **after** the upload lands, dispatches `dmg-verify` against the published tag (`-f dmg_tag=v<version>`), emits `ReleaseDmgNotarized` (which bumps the site link), then runs the deferred cleanup.

**The pending state is a manifest field, never a flag.** `release_staging_is_notarized` is the single question every public-facing consumer asks, so the banner, the site link and the cleanup cannot disagree with the bytes. An **absent** `notarized` key means notarized (the pre-2026-07-29 writer staged only after `Accepted`), and `restage_manifest_for_commit` carries the value forward — a restamp that dropped it would launder a deferred staging into a clean-looking one. `--defer-notarization` is refused on `--release` / `--release-attach`, which upload in the same process where no banner can be composed.

**Two accepted costs, both documented at their sites.** The updater tarball is built pre-staple, so early auto-updaters keep an unstapled (still Developer ID signed) bundle forever — invisible in practice, and unreachable by a later re-issue. And an `Invalid`/`Rejected` verdict now lands on an already-public asset: pull it with `gh release delete-asset`, leave the banner up, patch-release the fix. Deliberately not automated.

("Still Developer ID signed" was the *intent* of that first cost, and not the behaviour until 2026-08-02: see the next section. Pre-staple remains deliberate, because `--defer-notarization` never staples at all, so repacking after the staple would make the payload's contents depend on whether the release was deferred.)

### The updater payload is repacked from the SIGNED app (the v0.19.0 bug)

`cargo tauri build` packs `Lucidos.app.tar.gz` from the `.app` as the bundler leaves it, and `build-dmg.sh` runs that build with `APPLE_SIGNING_IDENTITY` stripped from the **subprocess** env on purpose (Tauri's codesign pass skips the ~200 loose Mach-O files in the Postgres tree, so the script signs the bundle itself, inside-out, afterwards). `refresh_dmg_payload` re-injects the signed app into the DMG, which is why the DMG was always correct. **Nothing did the same for the updater payload**, so every release from the introduction of the signed path through v0.19.0 shipped an ad-hoc updater bundle: `Signature=adhoc`, `TeamIdentifier=not set`, designated requirement `cdhash H"…"`, no `Contents/_CodeSignature`. Since a cdhash-anchored requirement changes with every build and macOS TCC keys grants on code identity, every auto-update silently destroyed every permission grant the user had made.

`scripts/lib/updater_payload.sh` (public-mirror-safe, offline-tested by `updater_payload_test.sh`) owns the fix, and `build-dmg.sh` wires it in at three points:

- **Repack + re-sign**, after `sign_app_bundle` and before `refresh_dmg_payload`. The `.sig` MUST be regenerated: a signature over the pre-repack bytes makes every updater reject the update. The signer is `tauri_signer_sign_file` in `tauri_signing_key.sh`, now the **single** `cargo tauri signer sign` call site (the release preflight's throwaway test-sign was refactored onto it, so the preflight validates the invocation the build actually makes).
- **A round-trip hard gate inside the repack**: extract what was just written and run `codesign --verify --deep --strict`, plus the three layout rules `tauri-plugin-updater`'s `entry.path().iter().skip(1)` + `Entry::unpack` impose. One top-level `.app` component; **no hard-link entries** (unpack resolves a hard link against the process CWD, and `bsdtar` emits them where Tauri's `tar` crate writer never did, so the hazard is one the repack introduces); no AppleDouble `._` entries. Packing uses `COPYFILE_DISABLE=1 tar --no-xattrs`: the second flag is what actually keeps xattrs (and with them a `PaxHeaders/…` entry per file, since macOS stamps `com.apple.provenance` on everything) out of the archive, keeping it the shape the updater has always been fed.
- **A publish gate at both chokepoints**, `stage_release_artifacts` and `upload_staged_assets`: extract the payload and assert a Developer ID designated requirement (`anchor apple generic` **and** `certificate leaf[subject.OU]`, not a bare `cdhash`) with a Team Identifier set. Both are fatal, and staging only ever runs release-grade, so this is the check that would have caught v0.19.0.

**The verdict is re-derived from the bytes, never recorded in the manifest.** A `updater_payload_signed` field would have to be carried forward by `release.sh::restage_manifest_for_commit` exactly like `notarized`, and a restamp that dropped it would launder an unsigned payload into a clean-looking one. Re-deriving costs one extraction and has no laundering path. The consequence to know: `--release-attach` now needs the system `codesign`, so it is macOS-bound (and, since the staple gate landed beside it, `xcrun stapler` too, which ships in the same Command Line Tools). That costs nothing (a release is macOS-only end to end and `require_release_signing_credentials` refuses a non-Darwin host), and it is why the attach path's "needs no Darwin tooling" comment now says "no build tooling".

**There is no verification of the regenerated `.sig`.** `cargo tauri signer` exposes only `sign` and `generate`, so nothing available can check a minisign signature against `plugins.updater.pubkey`. Standing in for it: the release preflight proves the key can sign at all, the signer fails loud on a non-zero exit, and `updater_payload_resign` refuses unless the new `.sig` is non-empty **and differs** from the pre-repack one, which is what catches a silent no-op. If a verifier ever becomes available, the honest place for it is beside the Developer ID check in `stage_release_artifacts`.

**Local builds get the same treatment one rung down.** With no `APPLE_SIGNING_IDENTITY`, `build-dmg.sh` signs the bundle with the stable dev identity from `scripts/lib/codesign.sh` when `lucidos_signing_identity_ready`, so a local `.app` rebuild stops discarding the developer's TCC grants. Strictly a fallback: an explicit identity wins, `--release*` still hard-requires Developer ID, the staging gate rejects a self-signed payload (no Team Identifier), and no notarization is attempted. It uses **neither** `--options runtime` nor `--timestamp`, deliberately: both exist for notarization, library validation under the hardened runtime matches by Team Identifier (which a self-signed cert lacks, risking the bundled Postgres dylibs failing to load), a secure timestamp would mean ~200 network round trips, and the certificate-anchored requirement depends on neither.

### A bundled helper process carries its own entitlements (ADR 0160)

`sign_app_bundle` signs every loose Mach-O with `--options runtime`. Entitlements
are per binary, so the outer `.app` signature governs the app process **and
nothing else**. The engine and the gateway are separate processes: a capability
either of them needs goes in its own plist, applied to that binary by name.

Two files today. `crates/lucidos-app/Entitlements.plist` is the outer `.app`
(the camera). `crates/lucidos-app/EngineEntitlements.plist` is
`Contents/Resources/lucidos-engine`, which embeds wasmtime and compiles a signer
module, so it claims `com.apple.security.cs.allow-unsigned-executable-memory`.
Release 0.32.0 shipped without it and macOS SIGKILLed the engine on every turn
that reached a Wasm-signed proxy.

**The gate is running the signed bytes, not reading them back.** `sign_app_bundle`
runs `lucidos-engine --wasm-selftest` as its LAST step, after the outer `.app`
codesign. So it exercises the bytes the function hands back, never an
intermediate artifact. A readback says what the plist asked for;
only this says what macOS allowed, and the difference is not hypothetical:
`allow-jit` reads fine and still crashes, because wasmtime never asks for a
`MAP_JIT` mapping. **Measure with a real Developer ID identity**, never `-s -`:
an ad-hoc signature does not honour these keys the same way, and believing it is
how the wrong key got picked first. Static halves are in `build_dmg_test.sh`.

### The submitted set is paired, and the manifest is published last (F3, F4, F5, F8, F10)

Five findings from `docs/audits/2026-08-02-macos-update-path-audit.md`, all in `build-dmg.sh` and its libs.

**The notarize handle pins a SET, not a file (F3).** It recorded `dmg_sha256` and nothing about the `.app.tar.gz` + `.sig`, which staging then picked up by glob from whatever was on disk. Nothing tied them to the build that produced the pinned DMG, and the recovery branch made that active rather than passive: it RESTORES the DMG from its pin after a concurrent rebuild, which is exactly the state in which the tarball beside it belongs to the newer build. The manifest recorded both, `release_staging_verify` found them self-consistent (it only ever compares staged bytes to the manifest the same run wrote), and the release shipped a DMG and an updater payload from two different builds. The 2026-07-28 three-concurrent-pollers incident is the precondition, so the tree has been in that state.

- The pairing is **captured at the first submission and carried forward** to the second (`notarize_capture_updater_pairing` / `notarize_carry_updater_pairing_forward`). Re-capturing at the second submit would let a tarball replaced during the first wait be adopted as the pairing: self-consistent again, and wrong again.
- **Every member is pinned**, each under its own content address (`.lucidos/notarize-submissions/<version>/<sha12>/`), so two concurrent builds of one version cannot collide. `cp -c` (APFS clonefile), never a hardlink: a hardlink is a second name for the same inode, so an in-place `codesign` rewrite corrupts the pin, and the suite proves it.
- `assert_submitted_artifacts_are_intact` **decides first and acts second**. Three `cp`s cannot be atomic, and they need not be; what must never happen is restoring SOME members and proceeding. It records what it would restore, copies nothing until every member is known intact or recoverable, and on any unrecoverable member dies having touched nothing, saying so. Each refusal `return`s as well as calling `die`, so the refusal is a property of that function rather than of `die`'s exit semantics.
- The gate runs before every irreversible step: before stapling, on both deferred branches, and at the top of `stage_release_artifacts`, which additionally refuses when the tarball it is about to stage is not the one the submission was paired with.
- **Our own staple is an intended mutation, and the gate has to know it (v0.19.1 Phase A).** `xcrun stapler staple` writes the ticket INTO the DMG, so the protected bytes change at the one moment the guard does not expect them to, and the gate could not tell that from a concurrent rebuild. The v0.19.1 run stapled the DMG, `stage_release_artifacts`' second assertion found the mismatch, located the pre-staple pin and copied it back over the ticket. The staple was silently undone, the manifest recorded the unstapled sha, and `release_staging_verify` found the pair self-consistent because it only ever compares staged bytes to the manifest the same run wrote. `spctl` still reported `accepted / source=Notarized Developer ID`, since that is an ONLINE lookup, so only the rc `dmg-verify` leg caught it (`stapler validate`, exit 65, *"does not have a ticket stapled to it"*). `notarize_record_stapled_dmg` now moves the expected bytes forward the instant the staple lands, and it **re-pins as well as re-records**: re-recording keeps a later rebuild detected, re-pinning keeps it recoverable by the STAPLED bytes rather than by a copy with no ticket. It sits in `staple_notarized_artifacts`, the one function both stapling paths funnel through (the fresh build and the `--attach-notarized` resume), so the two cannot drift. `--defer-notarization` never reaches it, which is correct: that path stages before anything is stapled, with `notarized: false` and its submission still in flight, so it keeps comparing against the submitted sha and keeps its pin and handle. Only the DMG needs it, because no later assertion reads the standalone `.app`'s bytes (the app stage checks its submitted zip, which stapling the bundle does not touch, and its cdhash, which the ticket does not change). Two things defend the moment the expected bytes move, because whatever gets recorded there is what the release stages: the DMG's ticket is **validated before its bytes are adopted** (and re-hashed afterwards to prove nothing moved during the check), which is the same proof the `.app` stage has always made after its own staple and the DMG half never did; and the ticket is **re-derived from the bytes at BOTH publish chokepoints**, `stage_release_artifacts` and `upload_staged_assets`, exactly like the Developer ID check beside it and for the same reason: `--release-attach` can be pointed at a staging dir an older `build-dmg.sh` wrote, and `run_release_attach`'s own pending check reads the manifest's `notarized` **flag**, which is precisely what said `true` over the unstapled v0.19.1 DMG. Both gates skip a deferred release, which stages and uploads unstapled by design and carries the pending banner. These gates are what turn the whole class loud: nothing else on the path says it out loud, because `spctl` resolves the ticket online. The **third** place the expectation lives is the *notarize resume handle*, and it moves at the same chokepoint (`notarize_carry_staple_into_handle`): the resume gate re-hashes `artifact_path` against `artifact_sha256` and refuses on any mismatch, so a handle left on the pre-staple bytes would make a just-stapled release unresumable, and on a deferred release that strands an already-published DMG no attach could ever staple. Pinned by section 9 of `release_staple_guard_test.sh`, which runs the real chokepoint against a fake `stapler` that mutates the file and then asserts the staged bytes and the manifest sha rather than any log line, plus the gate-before-copy ordering assertions in `build_dmg_test.sh`.

**Every DMG discovery excludes the refresh intermediates (F4).** `refresh_dmg_payload` writes `.rw.dmg` and `.zlib.dmg` beside the real artifact and a run killed mid-refresh leaves one behind; both match `*.dmg`, and the version-stamp guard cannot tell them apart because they carry the same version string. The adopt path had the exclusion and an arity check; the main discovery had `find … -name '*.dmg' | head -1`, which is directory order. `scripts/lib/release_dmg.sh` (offline-tested by `release_dmg_test.sh`) now owns the suffixes, so the code that WRITES them and the code that EXCLUDES them read one definition, and `release_dmg_find` refuses an ambiguous directory rather than picking arbitrarily. `refresh_dmg_payload` also clears both intermediates up front and unwinds its mount and its temp images on every failure branch, explicitly rather than through a `trap … RETURN` (under `set -e` a failing command does not return from a function, it exits through the ERR trap, and a RETURN trap never fires on that path).

**`latest.json` is attached last (F8).** All four assets went up in one `gh release upload`, which uploads concurrently, so the smallest file won. The updater reads `…/releases/latest/download/latest.json`, the release is already marked latest when the upload starts, and the manifest names a payload on the same tag: 10 s of 404 on v0.19.0, 65 s on v0.15.0, and **8h06m on v0.16.0**, during which the latest release carried no `latest.json` at all. `scripts/lib/release_upload.sh` (offline-tested with a fake `gh`) uploads the artifacts, re-reads the release and asserts each is present, `state == uploaded` and the right SIZE (a name appears the moment an upload starts, so size is the load-bearing half), then uploads the manifest in a second call. Bounded retry, fail-closed, and the manifest is a separate PARAMETER rather than the last artifact, so no argument list can put it back in the first batch. Both upload paths funnel through `upload_staged_assets`, so a corrective `--attach-notarized` re-upload cannot reopen the window.

**`latest.json`'s platform key describes the ARTIFACT (F10).** It came from a
`case "$(uname -m)"` in `upload_staged_assets`, which answers "what is this
machine" rather than "what is this payload". All ten sampled releases happened to
be right because the DMG is Apple-Silicon-only and the uploads ran on the build
Mac, but a `--release-attach` from a different host mislabels the payload, and
the mislabelling is **silent**: an updater whose target key is absent from
`platforms` reports *no update* rather than an error, so no client, log or gate
ever says anything. The key is now derived at BUILD time from the staged app
binary with `lipo -archs` (`release_staging_platform_key_for_binary`) and
recorded as `platform_key` in the staging manifest, which is also the only shape
that serves `--release-attach`, the path with no `.app` on disk. Three properties
hold it together:

- **`release_staging_verify` refuses a manifest without it**, and refuses an
  ABSENT key differently from an EMPTY one. Absent means a staging written before
  the recording existed, which a re-stage fixes; empty means a writer that
  recorded nothing, which a re-stage reproduces. That distinction is the same one
  `release_notarize_resumable` draws for a handle predating the pairing, and
  collapsing it would send an operator through a 40-minute rebuild to land on the
  identical manifest.
- **A universal binary is a hard error**, not two keys and not the first arch.
  The rest of the bundle is single-arch by construction (`stage_runtime_fetch_postgres`
  resolves one relocatable Postgres per target triple), so `darwin-x86_64` beside
  `darwin-aarch64` would advertise an Intel update whose bundled Postgres is
  arm64-only. The message names the two real answers: ship one release per
  architecture, or make the whole bundle universal first.
- **The generator cannot derive a key.** `release_upload_write_latest_json`
  (in `release_upload.sh`, moved there out of `build-dmg.sh` so it is unit-testable)
  takes the key as a PARAMETER and refuses an empty one; `release_upload_test.sh`
  asserts that nothing on that path consults `uname` at all. `release.sh`'s
  `restage_manifest_for_commit` carries the key forward like `notarized`, which
  cannot launder anything here because the key describes bytes the restamp leaves
  untouched.

**The `.app` inside the DMG is stapled (F5).** See ADR 0033: two notary submissions in Apple's order, the app's identity re-asserted by **cdhash** (what a ticket is issued for, and the only workable choice since `ditto -c -k` is not byte-reproducible), and the staple proved afterwards with both `stapler validate` and `codesign --verify --deep --strict`. The ticket lands in `Contents/CodeResources`, outside the sealed set at `Contents/_CodeSignature/CodeResources`, which is why stapling does not break the signature.

Banner + changelog-section text live in `scripts/lib/release_notes.sh` — **one** extractor shared by the publish and the attach step, so the body the attach step rewrites is byte-identical to the one the publish wrote. Offline-tested by `release_notes_test.sh` (banner content, the compose that never touches `$NOTES_FILE`, and that latest.json's notes stay plain) and the deferred sections of `build_dmg_test.sh` + `release_staging_test.sh`.

**Ask once per release: does this one need a *release notice*?** The changelog says what changed. A release notice says what the USER has to do about it. It is the only surface that reaches them unprompted, as a modal on their first open after upgrading. Nothing derives one, so a release that needed one and shipped without it never tells anybody: the drift it introduced sits in every workspace, silent. The test is narrow, and most releases fail it and correctly carry none.

Write one when the release leaves existing content or settings **stale rather than broken**, so nothing errors and nobody notices. A renamed event that stops a *trigger* firing, a schema an app should migrate off, a setting that now wants configuring. Not for a new feature, which the changelog covers. Author it in `release-notices.toml` at the repo root, whose header owns the field rules, with `since` set to the release in hand. Adding one is an ordinary change on `main` before the cut, never a step inside `release.sh`.

## Installer (`install.sh` + `uninstall.sh`)

`install.sh` is the user-facing `curl … | sh` installer (steps 3 + 4 of `docs/plans/2026-06-30-installer-step3-download-and-run.md` + `docs/plans/2026-06-30-installer-step4-service-mode.md`). **Three modes:**

- **(default) download-and-run + register a service** — detect the host triple (the SAME `stage_runtime_host_triple` map the build scripts use — no divergent mapping), resolve the version, `curl` the prebuilt `lucidos-<version>-<triple>.tar.gz` + `.sha256`, **verify the checksum (mandatory, fail-closed)**, extract to the SHARED `$LUCIDOS_PREFIX/runtime/<stem>/`, then **register the bundled gateway as a user-level service** so it survives terminal-close + reboot and restarts on failure. The service runs `lucidos-gateway` directly with the SAME env `crates/lucidos-app/src/desktop.rs::spawn_gateway` sets (`LUCIDOS_GATEWAY_PG_BACKEND=embedded`, `LUCIDOS_PG_BIN_DIR`/`LUCIDOS_PG_LIB_DIR`, `LUCIDOS_ENGINE_BIN`, `LUCIDOS_STATIC_DIR`, `LUCIDOS_SDK_DIR`, `LUCIDOS_SYSTEM_KNOWHOW_DIR`, `FASTEMBED_CACHE_DIR`, `LUCIDOS_BOOT_WITHOUT_PROVIDER=1`, `LUCIDOS_PACKAGED=1`) — emitted once by the pure `service_runtime_env_pairs`, shared by the foreground launch + the plist + the unit. `--no-service` (`LUCIDOS_NO_SERVICE`) runs the gateway in the **foreground** instead (the step-3 behavior). No Docker/Rust/Node/clone/compile.
- **`--dev` / `--source` / `LUCIDOS_FROM_SOURCE=1`** — the legacy compile-from-source path, preserved verbatim (toolchain bootstrap, clone/update, `data/.env`, build + launch via `scripts/run.sh`). The only network/compile path; **always foreground** (never registers a service).
- **`--from-tarball <path>`** — install a LOCAL tarball (offline; e.g. from `build-headless.sh`). Verifies the adjacent `<path>.sha256` if present (fail-closed), warns if absent, extracts, and registers the service too (unless `--no-service`).

**Service = the GATEWAY only (ADR 0014).** The service supervises the gateway; the gateway provisions the embedded Postgres + spawns/supervises the engines itself — never a service per engine. The gateway ignores SIGTERM and stops gracefully on SIGUSR1 (`crates/lucidos-gateway/src/server.rs`), so the systemd unit sets `KillSignal=SIGUSR1` + `KillMode=process` (stop the gateway; leave engines + PG for a relaunch to re-adopt).

**Slug-keyed multi-instance.** Several gateways coexist as named *instances* (`--name <slug>` / `LUCIDOS_INSTANCE`, default `default`). The **port is a mutable property**, not the identity, so a re-run with a new `--port` moves an instance. Each instance owns `<prefix>/<slug>/` (registry + embedded PG + `fastembed/` + `logs/` + a `port` marker) and a slug-suffixed service id; the **runtime is downloaded once and SHARED** at `<prefix>/runtime/current`. Slugs `gateway`/`runtime`/`current`/`logs` are reserved (so a `--name` can't alias the dev gateway's `~/.lucidos/gateway` or the shared runtime). This is how a terminal install coexists with a dev gateway (5251) and the packaged `.app` (5252). **Service ids + paths:** launchd `com.lucidos.gateway.<slug>` at `~/Library/LaunchAgents/` (logs `<prefix>/<slug>/logs/gateway.{out,err}.log`); systemd `lucidos-gateway-<slug>.service` at `${XDG_CONFIG_HOME:-~/.config}/systemd/user/` (logs `journalctl --user -u lucidos-gateway-<slug>`).

**Port resolution (idempotent; port is changeable).** Pinned `--port P`: use P if free or already this instance's, else **fail closed** (a foreigner holds it). Bare on an existing instance: reuse its recorded `<data>/port`. Bare on a NEW instance: auto-pick the first free port from 5252 up (stepping around a running `.app`). After registering, a **health check** polls `http(s)://localhost:<port>/~/api/v1/health` (`LUCIDOS_HEALTH_TIMEOUT`, default 120s; `curl -k`, scheme follows the TLS opt-in) and fails loud with a logs hint if it never answers.

**TLS opt-in (`--tls-cert`/`--tls-key`, env `LUCIDOS_TLS_CERT`/`LUCIDOS_TLS_KEY`).** Both-or-neither, files must exist (fail closed). When supplied, the pairs are appended to the service/foreground env (`service_tls_env_pairs`) so the bundled gateway serves **https**, which is what gives a NON-localhost device a secure context (service worker, PWA install, web push all require one; plain http limits them to localhost). Works with `tailscale cert` / mkcert / CA certs. Engines still never see `LUCIDOS_TLS_*` (the gateway strips them: it terminates TLS, ADR 0014), and `restart_via_gateway` tolerates the scheme mismatch via `peer_scheme_order()`. Remote reachability is separate (`--bind` below, or Settings → Access → Network access; loopback-only default unchanged). Like provider creds, TLS is baked from THAT run's flags: a re-run without them reverts the service to plain http.

**macOS CLT preflight (download / from-tarball paths).** `install.sh` probes `xcode-select -p` on Darwin and **warns (never dies)** when the Command Line Tools are absent: chat works, but coding agents / Apply / `run_python` shell out to git + python3, whose `/usr/bin` shims error until CLT is installed. The engine mirrors this at boot (`git_preflight` + `python_preflight` in `main.rs`, warn-only) and startup-augments its own process PATH with the common user-install bin dirs (`core::user_path::augment_process_path` — Homebrew, `/usr/local/bin`, `~/.local/bin`, npm-global; dedupe ⇒ no-op on a dev shell PATH) so bare-name tools (`claude`/`codex` fallbacks, chat bash/python shell-outs, stdio MCP servers) resolve under the launchd minimal PATH exactly as in dev. Agent children additionally get the bundled `LUCIDOS_PG_BIN_DIR` PATH-prepended (`spawn_env::agent_path_prefixes`) so the advertised bare `psql -c '…'` works inside coding-agent threads on a packaged install, mirroring what `workspace_script_env_vars` already did for chat bash/python tools.

**Manager detection + degrade.** macOS → launchd; Linux → systemd `--user` (probed via `systemctl --user show-environment`) + best-effort `loginctl enable-linger` (announced, never hard-fails). **No supported manager** (e.g. a container) → **degrade to a foreground launch** with a clear message, never fail.

**Post-extract validation + preflights.** `finish_install` runs the extracted `lucidos-gateway --build-id` once — the **execution smoke**, so a too-old glibc / wrong-arch tarball fails AT INSTALL TIME with a distro-floor message pointing at `--dev`, instead of an opaque service crash-loop. Then it warns — never fails — about missing host runtime deps: `git` (the engine shells out for every git op) and, on Linux, a system CA bundle (candidate list = `install_ca_bundle_candidates` in `install_common.sh`; rustls reads the system store for LLM/model/web-push TLS).

**Remote access (`--bind`) + unit-value escaping.** Default posture stays loopback + plain http; the final banners print the remote options (SSH tunnel and `tailscale serve` keep a SECURE origin — which web push + PWA require — with zero config), and the https half is the TLS opt-in above (`append_tls_env` is ADDITIVE, so the flag-less env block stays byte-identical to `spawn_gateway`'s contract). `--bind all|loopback|<IP>` (`LUCIDOS_BIND`) writes the machine-global `~/.lucidos/network.toml` via `service_write_network_toml` (byte-mirror of the gateway's own writer, preserves `[engine] inherit`) — **never unit env**, which would permanently shadow the picker's Settings → Network access knob (env beats the file). Invalid `--bind` values are refused up front (the gateway would silently fall back to loopback). systemd unit values are escaped via `service_systemd_escape_env` (`%%`, `\"`, `\\` — an API key with `%` used to reach the gateway mangled); launchd's twin is `service_xml_escape`.

**Uninstall.** `uninstall.sh` (and `install.sh --uninstall`, which delegates to it): `--name <slug>` removes one instance (a bare uninstall removes the sole instance, else lists), `--all` removes every instance, `--list` shows instances + ports. It stops + unregisters the service (both launchd + systemd artifacts that exist), gracefully stops that instance's engines + embedded Postgres, and **keeps all data unless `--purge`** (prints what it left). `--purge` deletes the instance data dir; `--all --purge` also deletes the shared runtime. The systemd unit FILE is removed **even when the user D-Bus session is unreachable** (bare ssh, no `XDG_RUNTIME_DIR`) so an "uninstalled" service can't resurrect at the next boot; in that case the possibly-running stack is left alone (a bus-less shell can't stop the gateway, and killing its engines would only make it respawn them).

**Discovery is the `<prefix>/<slug>/port` marker, and BOTH launch shapes write it.** `service_list_instance_names` lists exactly the `<prefix>/*/` dirs carrying one, so that file is the whole of "is this instance installed": no marker means invisible to `--list`, no target for `--all`, and `run_uninstall` returning before the purge, which leaves the data dir *and* the shared runtime on disk. Until 2026-07-30 only `register_service` wrote it, so a `--no-service` run or the no-manager degrade (a container) finished uninstallable. Both paths now go through `record_instance_port`, and the **orderings stay deliberately different**: `register_service` writes *after* its unit, so a failed registration leaves no marker, while `launch_runtime` writes *before* an `exec` that never returns. `service_test.sh` asserts the marker and `service_list_instance_names` discoverability on both foreground paths, and `install-smoke.yml`'s front-door rungs 5-8 assert the end-to-end consequence against the live origin.

**Shared logic, one source of truth.** install.sh **sources** `scripts/lib/{stage_runtime,headless_tarball,install_common}.sh` (triple/stem/URL) and `scripts/lib/service.sh` (service templating/detection) from `<self>/scripts/lib` when run from a checkout; when piped it **fetches** those small pure libs from the same ref (`${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib`, overridable via `LUCIDOS_LIB_BASE_URL`) — never re-implementing any map.

**A fetched lib is content-sniffed before it is sourced, fail-closed.** `curl -fsSL` plus a non-empty test cannot see a **soft-404**: an origin that answers an unknown path with its landing page and a **200** makes both checks pass, and `.` then executes HTML as shell. That shipped: a clean `ubuntu:22.04` running the advertised one-liner on 2026-07-29 died on ``stage_runtime.sh: line 1: `<!DOCTYPE html>` `` because the Cloudflare Pages SPA fallback served the landing page for `scripts/lib/*.sh`. `_source_libs` therefore rejects a payload whose first non-blank line opens a tag (`<!DOCTYPE`, `<html`, `<?xml`), naming the lib + origin and pointing at a checkout or `--dev`. The missing-file half was fixed at the publisher (it now uploads the libs beside `install.sh`); **the sniff is the defence in depth and stays**, because a wrong or hijacked origin can still soft-404. Covered by `install_test.sh` in both directions: an HTML payload for every lib is refused and never reaches the shell, and the real libs fetched over `file://` still install cleanly.

**Every place a fetched payload reaches a shell is sniffed, and there are five.** `_source_libs` was long described here as "the one place unknown remote content reaches `source`", which was never true and hid its siblings until the 2026-07-30 docs audit. The full set, all fail-closed, all covered by `install_test.sh`:

| site | file | reaches the shell via | test |
|---|---|---|---|
| helper libs | `install.sh` `_source_libs` | `.` (source) | HTML payload per lib, plus the real libs over `file://` |
| dash re-exec | `install.sh` bootstrap guard | `exec bash -c` | HTML re-fetch refused; the version-pinning stub carries a shebang |
| fetched uninstaller | `install.sh` `dispatch_uninstall` | `exec bash -c` | `--uninstall` and `--list` against an HTML origin, plus the real uninstaller over `file://` |
| fetched `service.sh` | `uninstall.sh` `source_service_lib` | `.` (source) | HTML `service.sh` refused |
| dash re-exec | `uninstall.sh` bootstrap guard | `exec bash -c` | mirror of install.sh's; same shape |

The four added on 2026-07-30 assert the payload **starts with `#!`** (fail-closed, and the same test the front-door CI rung applies to what the origin serves); `_source_libs` keeps its leading-`<` rejection because its message and tests name the individual lib being refused. Both re-exec guards must stay POSIX sh: they run before bash is guaranteed. **`uninstall.sh`'s lib fetch is the likeliest of the five to actually meet a soft-404**, because it derives its lib base from `${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib`, i.e. from the origin rather than from a checkout. Production has served a real `uninstall.sh` since 2026-07-31, so this is now a guard against a route regressing rather than a known-missing one.

`install_common.sh` holds the pure URL/version/dir helpers; `service.sh` splits **PURE** helpers (identity, paths, plist/unit templating, manager DECISION + compose decision, env pairs, slug/port validation, port candidates, uninstall paths) from thin **EFFECTFUL** wrappers (launchctl/systemctl/curl/kill/pg_ctl calls, port probing, instance listing): the offline tests exercise the pure ones directly, and reach the effectful ones only through fakes on `PATH`, never a real service manager.

**The launchd wrappers are the one effectful pair the tests DO drive, because
their contract is a timing one.** `launchctl bootout` is asynchronous: it returns
0 when launchd *accepts* the request, and the job stays visible to `launchctl
print` until the teardown finishes, ~5s for the gateway (it ignores SIGTERM, so
launchd has to wait out `ExitTimeOut` and SIGKILL it). Both wrappers therefore
decide by OBSERVING the domain through `service_launchd_wait_gone`, never by
launchctl's exit code, and `LUCIDOS_LAUNCHD_TIMEOUT` (default 30s, headroom over
launchd's documented 20s `ExitTimeOut`) bounds the wait. Trusting the exit code
had shipped two bugs: `uninstall.sh` reported *"Stopped launchd agent"* over a
still-bootstrapped KeepAlive job (what install-smoke's front-door rung 7 catches),
and `service_launchd_load` bootstrapped into a domain that still held the dying
job, which launchd refuses with *"Bootstrap failed: 5: Input/output error"*, so a
re-run over a running instance reported success and left NO job at all once the
old one died. `service_test.sh` pins both with a **stateful fake launchctl** that
models the async teardown; a fake with a fixed `print` exit code cannot express
the bug, which is why the original one did not catch it.

**Version resolution — the baked default is what the public one-liner uses.** `install_resolve_version` takes `--version`/`LUCIDOS_VERSION` → the `RELEASE` file **next to the script** → the baked `LUCIDOS_DEFAULT_VERSION` in `install.sh`. A piped `curl -fsSL https://lucidos.dev/install.sh | sh` has no checkout and therefore no adjacent `RELEASE`, so **every public install lands on the baked constant** — RELEASE only reaches an install run from a checkout. That made a stale constant a shipped outage, not a cosmetic lag: 0.14.0 predates headless tarballs, so the advertised one-liner 404'd. Two things keep them in lockstep: `release.sh` rewrites the assignment (anchored to line start, failing loud if the pattern doesn't match) in the same step that bumps `RELEASE`, and commits `install.sh` with it; and `install_test.sh` asserts the parsed constant equals the repo-root `RELEASE`, so a hand-edit — or a removed substitution — fails a test rather than a user's install.

**One source of truth for the version — `RELEASE`, and enforcement.** Everything else DERIVES from it: `build.rs` reads it at build time, `release.sh` rewrites `install.sh`'s baked constant at release time, and the dev workspace's site publisher pins the landing page's download links at publish time. A second hand-maintained copy always drifts — the baked constant did (a shipped 404), and CONTRIBUTING.md and PRIVACY.md both announced the "0.9.x line" long after main left it. `scripts/lib/version_sources_test.sh` enforces this: it scans the tracked tree for the current version anywhere nothing keeps in sync, pins both halves of the install.sh mechanism (the equality AND release.sh's rewrite, so deleting the substitution fails now rather than at the next bump), and flags prose that announces which release line the project is on. Historical narration is deliberately exempt — CHANGELOG, `docs/plans`, `docs/adr`, and text explaining that "0.14.0 predates headless tarballs" are correct precisely because they don't move; only claims about the CURRENT version rot. Phase A runs the suite against the WORKTREE right after both bumps, so a stale literal fails the release instead of shipping. Three gotchas are documented at their sites in the suite: `git grep -E` is POSIX ERE where `\b` is **not** a word boundary (use `-w`), a pathspec of only `:(exclude)` matches **nothing** (needs a leading `'.'`), and macOS bash 3.2 has no `mapfile`.

**Layout.** `LUCIDOS_PREFIX` (default `$HOME/.lucidos`) → shared runtime at `<prefix>/runtime/lucidos-<version>-<triple>/` + a `<prefix>/runtime/current` symlink; per-instance data at `<prefix>/<slug>/` (override the single-instance data dir with `LUCIDOS_GATEWAY_DATA`). **Idempotent:** an already-extracted runtime for the target version isn't re-downloaded/re-extracted unless `--force` (`LUCIDOS_FORCE`). `--no-launch` (`LUCIDOS_NO_LAUNCH`) installs without starting or registering.

**Env/flags:** `--name`/`LUCIDOS_INSTANCE`, `--version`/`LUCIDOS_VERSION`, `--base-url`/`LUCIDOS_RELEASE_BASE_URL` (default `https://github.com/lucidos-dev/lucidos/releases/download/v<version>`), `--prefix`/`LUCIDOS_PREFIX`, `--port`/`LUCIDOS_PORT` (default 5252), `--bind`/`LUCIDOS_BIND`, `--tls-cert`/`LUCIDOS_TLS_CERT` + `--tls-key`/`LUCIDOS_TLS_KEY` (https opt-in), `--no-service`/`LUCIDOS_NO_SERVICE`, `--force`/`LUCIDOS_FORCE`, `--no-launch`/`LUCIDOS_NO_LAUNCH`, `--uninstall`/`--list`/`--all`/`--purge`, `LUCIDOS_HEALTH_TIMEOUT`, `LUCIDOS_LAUNCHD_TIMEOUT` (macOS, default 30s: how long an unload waits for a booted-out job to actually leave the domain). Provider creds (`OPENAI_API_KEY`/`VERTEX_PROJECT_ID`/`VERTEX_REGION`) are exported into the foreground gateway and **baked into the service env (mode 600)** when supplied. The env-as-flag contract means a dev shell that exports `LUCIDOS_TLS_CERT/KEY` (every engine-spawned subprocess does) silently configures TLS on a manual install run; the offline test suites `unset` them.

**Every published Release carries the four per-platform tarballs, at the moment it becomes visible (2026-08-04).** `release-tarballs.yml` attaches them to the release while it is still a DRAFT, and the publish waits for all four, so the old ~30 minute window in which a brand-new version 404s is gone. `download_failed` no longer names it and now names the causes that remain: a platform that was never published (a release cut with `--allow-missing-tarballs`, or an asset removed since), or the network. Offline-tested by `scripts/lib/install_test.sh` (download/extract path) and `scripts/lib/service_test.sh` (service.sh pure helpers + the foreground/degrade/register/uninstall wiring, all faked, with no real launchd or systemd).
