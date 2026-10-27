---
paths:
  - "scripts/build*.sh"
  - "scripts/check-staged-copies.sh"
  - "scripts/release*.sh"
  - "scripts/lib/build_*.sh"
  - "scripts/lib/stage_runtime*.sh"
  - "scripts/lib/stage_runtime*.py"
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
  # The release gate on the one version source, described under "One source of
  # truth for the version" below.
  - "scripts/lib/version_sources_test.sh"
  - "install.sh"
  - "uninstall.sh"
  - "rust-toolchain.toml"
  - ".github/workflows/release-tarballs.yml"
  - ".github/workflows/install-smoke.yml"
  - ".github/workflows/e2e.yml"
  - "scripts/check-e2e-workflow.sh"
  - "scripts/lib/check_e2e_workflow_test.sh"
  # The mirror's one-door gate (ADR 0462), described under "The mirror has one
  # door" below. release_tree_push_test.sh matches scripts/lib/release_*.sh.
  - "scripts/check-mirror-pushers.sh"
  - "scripts/mirror-push-gate.sh"
  - "scripts/lib/check_mirror_pushers_test.sh"
  # The local rc channel, described under "Test an update before publishing".
  - "scripts/publish-rc-channel.sh"
  - "scripts/lib/rc_channel*.sh"
---

# Build, Packaging & Installer

Building the engine and desktop app, lockfile determinism, runtime staging, the
release pipeline and the `curl … | sh` installer. The dev-runtime half is
`.claude/rules/dev-runtime.md`, and the front-door CI jobs are
`.claude/rules/front-door.md`. Facts that no ADR or script header holds live in
`docs/release-pipeline-notes.md`.

**`paths:` above is an explicit list, not a `scripts/**` catch-all.** A new script
under `scripts/` gets no rule until its path is added here or to `dev-runtime.md`.
A path may sit in both lists (`codesign.sh` does). `Makefile` belongs to
`dev-runtime.md` alone.

## Build

```bash
cargo build -p lucidos-engine --release    # Engine
./scripts/build-dmg.sh [--emit-tarball]     # macOS: self-contained .app + .dmg (bundled PG)
./scripts/build-headless.sh [--check]       # Linux + macOS: Tauri-free headless tarball
```

### GitHub Actions never gates a change: release checks, on-demand e2e and the main check

- **A workflow verifies a release or delivery artifact, runs e2e on request, or
  runs the post-land main check.** That is the full list for `e2e.yml`,
  `install-smoke.yml` and `release-tarballs.yml`. Their headers say what fires
  each (ADRs 0382, 0386, 0464, 0536).
- **Never add a workflow that compiles, lints, type-checks or tests per push or
  PR.** Lucidos is not PR-based, so such a trigger never fires or fires too late.
  A per-change check goes into `scripts/lib/harden_suites.sh` (`hs_select_suites`, `hs_suite_command`).
- **Nothing in a workflow deploys.** Deploys run on the maintainer's machine,
  because a Cloudflare token that can deploy Pages can also edit the zone's DNS
  (ADR 0031).
- **The main check gates nothing.** `scripts/main-check.sh <sha>` runs after a
  landing, and a red one opens one fix thread. `/harden` reads it with
  `lucidos main-check status`.
- **Each `install-smoke.yml` cron verifies something external.** Every job guards
  on `github.event.schedule`, or the file's other jobs multiply their frequency.

### Toolchain pin: `rust-toolchain.toml` is authoritative

- **`Cargo.lock` pins what is compiled, the toolchain pin pins what compiles it.**
  It fixes clippy's lint set and rustfmt's output (ADR 0030), and declares the
  wasm32 target `signers/build-all.sh` needs.
- **Bumping the pin is its own commit.** Run `make lint` and the engine suite, fix
  what the new lint set surfaces, and run `make fmt` in the same change.
- **Never install a toolchain ahead of the pin in CI.** Containers install rustup
  with `--default-toolchain none`.

### CI caches DEPENDENCIES only, and the release legs sit in their own namespace

A release build may use `Swatinem/rust-cache` only because it drops workspace
members before saving, so every release compiles `crates/**` from source. Four
rules, each asserted by `release_tarballs_gate_test.sh`:

- **Pin the action by commit, never by tag.**
- **Key per leg** (triple and container image).
- **Only the `rc/**` arm writes** (`save-if`).
- **`prefix-key` fences release from smoke.** Nothing `install-smoke` writes may be
  readable by `release-tarballs`.

### Lockfile determinism: builds are fail-closed (ADR 0020)

- **Every cargo build, test, check, clippy and run in `scripts/**` and `Makefile`
  passes `--locked`.** Every npm install site uses `npm ci`, never `npm install`.
- **A dependency changes only through a deliberate `cargo update` or
  `npm install <pkg>` that commits the lockfile.** Never drop `--locked` to fix a
  lock failure. Manifests keep caret ranges, not exact `=` pins.
- **A coding-agent worktree gets `node_modules` at spawn.** Never reinstall it.

### Shared staging and the resource contract (ADR 0121)

- **`scripts/lib/stage_runtime.sh` stages the runtime for both build paths.** It
  takes `<name>=<path>` pairs and does not source the contract lib, since a piped
  `install.sh` fetches it over the network.
- **`scripts/lib/resource_contract.sh` is the one list.** Never restate
  `RESOURCE_NAMES`, `BUNDLED_EXECUTABLES` or the Tauri resource map.
  `resource_contract_check` asserts a three-way set equality against both runtime
  launchers (`service.sh`, `desktop.rs::spawn_gateway`). A new resource is added in
  one place plus the launcher that resolves it.
- **`scripts/check-staged-copies.sh` guards a stale stage** from
  `tauri.conf.json`'s `beforeBuildCommand`. An absent stage is clean.
- **The `lucidos` CLI is load-bearing.** The engine finds it beside
  `lucidos-engine` or through `LUCIDOS_CLI_BIN`.
- **Headless tarballs.** `build-dmg.sh --emit-tarball` emits the signed one and
  `build-headless.sh` the unsigned one, both through
  `scripts/lib/headless_tarball.sh`.
- **Linux tarballs via CI (`release-tarballs.yml`).** Linux builds inside the glibc
  2.35 floor container, and only the tag push attaches, to a DRAFT release. The
  attach step never deletes an asset from a published release. The macOS tarballs
  on a Release are unsigned, because CI cannot hold the Developer ID identity
  (ADR 0034). The workflow header holds the rest.

### The release candidate IS the published artifact (rc-first, ADR 0024)

A release is one stripped tree, built once, tested, then promoted. Never validate
one tree and ship another.

- **`scripts/lib/release_tree.sh` is the one strip implementation.** It owns
  `RELEASE_TREE_EXCLUDE_PATHS`, the fail-closed `release_tree_scan` and the
  deterministic `release_tree_commit`. `release-to-lucidos.sh` has no exclusion
  list of its own.
- **Phase A (`release.sh --verify-build`) builds the candidate and arms the gate.**
  It scans, commits onto the mirror's `main` lineage and force-pushes
  `rc/<version>` before the DMG build, then records `RC_COMMIT` and `RC_PARENT`.
- **The `rc-<version>` release is a draft, and `dmg-verify` is dispatched
  explicitly.** Never add `created` to its `release: types:` (ADR 0036).
- **A dispatch failure must print gh's own words.** `dispatch_dmg_verify` classifies
  through `release_dispatch_failure_class`.
- **Phase B (`--publish-verified`) promotes and never rebuilds.**
  `release_promote_preflight` refuses a moved rc or mirror `main`, a missing
  `RC_PARENT`, a `source_commit` mismatch and drifted hashes.
  `release-to-lucidos.sh --promote-rc` pushes that same object, tags by SHA,
  creates a DRAFT Release, waits for the tarballs and publishes. `release.sh` then
  settles the source side before emitting `LucidosReleased` (ADRs 0250, 0042).
- **The legacy one-shot has no rc gate**, and is held to both ADR 0039 rules.

#### The mirror's `main` is a linear release history (ADR 0039)

Release N's commit has release N-1's as its only parent. That parent must be the
object the mirror's `main` already holds, because a push sends every reachable
object while the scan inspects only the tip. All of it is in `release_tree.sh`:

- **`release_tree_ancestry_is_published` allows one parent**, exactly the mirror's
  `main`. **`RC_PARENT` is decided once**, in Phase A.
- **`release_main_push_decision` returns `published`, `lease`, `create` or
  `refuse`.** `lease` pushes with `--force-with-lease=refs/heads/main:<parent>`.
- **`release_mirror_history_check` runs on every publish path** (Phase A, Phase B's
  guard, the one-shot). It refuses both fewer and more commits than tags, and
  takes an in-flight count that is 0 except for a retry. Never `git fetch --tags`
  here (ADR 0029).
- **`RELEASE_MIRROR_HISTORY_EXCEPTIONS` is re-verified against the live mirror
  every run.** A stale entry refuses, the SHA is the full 40 hex, and a human adds
  entries (`docs/temporary-measures.md`).

#### The mirror has one door: `release_tree_push` (ADR 0462)

Every write to the public mirror goes through `release_tree_push` in
`scripts/lib/release_tree.sh`. It refuses unless the ref is a release or e2e ref
(`main`, `rc/*`, `e2e/*`, `v*`), the tree passes the scan and the ancestry is
published. `release_mirror_gate` points `remote.lucidos.pushurl` at a refusing
value, installed by `scripts/mirror-push-gate.sh`. `./scripts/check-mirror-pushers.sh`
fails `/harden` when any other tracked script pushes. Route the push through
`release_tree_push`, never widen the allowlist.

### The source side: main gets the bump, and the tag names it (ADR 0029)

- **The mirror's `v<version>` names the stripped commit, pushed by SHA.** The local
  and `origin` tags name the release commit on `main`.
- **`scripts/lib/release_main_sync.sh` lands the bump on `main`** through
  `settle_and_announce_release`: fast-forward, merge when `main` moved, or
  hard-fail after `merge --abort`. The tag names the release commit, never a
  cherry-pick.
- **`LucidosReleased` fires only after the landing, tag and `origin` push.**
- **Skips reprint at the end in a `STILL OWED` block.** `origin` is never forced.
- **`PREV_TAG` is a semver sort, never `git describe`.**

### Notarization is resumable: never a foreground `--wait`

- **`build-dmg.sh` submits with `--no-wait` and persists the handle first**
  (`.lucidos/release-state/notarize-<version>.json`, `release_notarize.sh`). A
  killed waiter must cost a poll, not a rebuild.
- **One handle carries both submissions (ADR 0033).** `stage` is `app` or `dmg`,
  and every key is always written.
- **Resume with `release.sh --resume-notarize <version>`.** Adopt an unrecorded
  submission with `--adopt-submission` or `--adopt-app-submission`, never both.
- **The resume gate is strict (`release_notarize_resumable`).** Fields, hashes and
  `source_commit` must match, or the manifest would claim a commit the DMG never
  came from.
- **Credentials go through `notarytool_run`.** Never `--password` in argv, never
  `store-credentials`.

### Preparing a release unattended: preflight, deadline, abandon

None of `--prep-preflight`, `--notarize-deadline` or `--abandon` publishes, pushes
to `main`, creates a `v*` tag or publishes a Release. Preserve that.

- **`--prep-preflight` is read-only.** Exit 0 is GO, 10 SKIP, 1 ERROR. An
  unreadable mirror fails closed to skip.
- **`--notarize-deadline` stages nothing on expiry.** It replaces
  `NOTARIZE_POLL_TIMEOUT` and is refused with `--defer-notarization`.
- **`--abandon` refuses once `v<version>` is published.**
- **Both Phase A entry points call `run_release_build_dmg`**, the one reader of
  build-dmg.sh's exit status.

### The release does not wait on Apple: deferred DMG (ADR 0027)

- **`--defer-notarization` defers only the DMG's verdict.** Phase A stages an
  unstapled DMG, Phase B publishes with the pending banner and keeps the staging
  inputs, and `release.sh --attach-notarized <version>` staples and re-stages.
- **The pending state is a manifest field, never a flag.**
  `release_staging_is_notarized` is the one question, and an absent key means
  notarized.

### The updater payload is repacked from the SIGNED app (the v0.19.0 bug)

`scripts/lib/updater_payload.sh` repacks the signed app:

- **Repack and re-sign after `sign_app_bundle`**, through `tauri_signer_sign_file`.
- **A round-trip gate and a publish gate check the bytes.** The publish gate asserts
  a Developer ID designated requirement at `stage_release_artifacts` and
  `upload_staged_assets`.
- **The verdict is re-derived from the bytes, never recorded in the manifest.**
- **Local builds fall back to the dev identity** in `scripts/lib/codesign.sh`.

### Signing, pairing and publish order

- **A bundled helper carries its own entitlements (ADR 0160).** The gate is
  running the signed bytes. See `docs/release-pipeline-notes.md`.
- **The notarize handle pins a SET.** The updater pairing is captured once,
  pinned with `cp -c`, never a hardlink, and checked before every irreversible
  step.
- **Every DMG discovery excludes `.rw.dmg` and `.zlib.dmg`.**
- **`latest.json` is attached last**, and its platform key comes from
  `lipo -archs` at build time, recorded as `platform_key`.
- **Banner and changelog text come from `scripts/lib/release_notes.sh`.**
- **Ask once per release whether it needs a release notice.** See
  `docs/release-pipeline-notes.md`.

### Test an update before publishing: the rc channel

- **`scripts/publish-rc-channel.sh <version>` is by hand only.** Phase A names
  it in its handoff and never runs it.
- **It writes only `<workspace>/data/artifacts/releases/rc/`.** It reads the
  staging and never touches `~/.lucidos` or the app's data.
  `scripts/lib/rc_channel_test.sh` asserts both.
- **The bundle keeps its real signature.** The update address changes where the
  app looks, never which key it trusts. Steps: `docs/desktop-app.md`.

## Installer (`install.sh` + `uninstall.sh`)

- **The default path verifies the checksum, fail-closed.** `--dev` compiles from
  source in the foreground. `--from-tarball` installs offline.
- **Both front doors stay inside the bash-3.2 POSIX subset.** Only
  `scripts/lib/install_test.sh` enforces it.
- **The service is the gateway only (ADR 0014).** It stops on SIGUSR1, not SIGTERM.
- **Instances are slug-keyed**, and the port is a mutable property. A pinned
  `--port` fails closed on a foreign holder. `<prefix>/<slug>/port` is the
  discovery marker.
- **TLS is opt-in and both-or-neither.** The default is loopback plain http.
- **Warn, never die:** the macOS CLT preflight. **Degrade, never fail:** no
  service manager means a foreground launch.
- **`finish_install` runs `lucidos-gateway --build-id` once**, so a bad glibc
  fails at install time.
- **Uninstall keeps data unless purged.** Neither uninstaller removes the other
  vehicle (ADR 0189).
- **A fetched payload is content-sniffed before it reaches a shell, fail-closed.**
  Five sites do it (`docs/release-pipeline-notes.md`). A new site gets a sniff and
  a test.
- **`service.sh` splits pure helpers from effectful wrappers.** The launchd
  wrappers decide by observing `service_launchd_wait_gone`, never launchctl's exit
  code.
- **A piped one-liner installs the baked `LUCIDOS_DEFAULT_VERSION`**, since it has
  no `RELEASE` file.

### One source of truth for the version

`RELEASE` is the one source. `build.rs` reads it, `release.sh` rewrites
`install.sh`'s baked constant in the same step that bumps it, and the site
publisher pins the download links. `scripts/lib/version_sources_test.sh` enforces
it, and Phase A runs it against the worktree after both bumps. Never hand-maintain
a second copy.
