# Desktop app (.dmg) — build & release runbook

The macOS desktop app is a **self-contained** Tauri bundle: it ships PostgreSQL +
pgvector, the standalone `lucidos-gateway` binary, the `lucidos-engine` binary,
the `lucidos` CLI binary (backs the coding-agent permission/question MCP servers,
the CC hooks, and chat-script `lucidos …` calls), the JS SDK, and the built
frontend inside the `.app`, so an end user double-clicks the `.dmg`, drags
Lucidos to Applications, and launches — no terminal, no Docker, no dev tools. It
auto-updates from GitHub Releases.

Architecture and the *why* behind each choice: [ADR 0012](adr/0012-self-contained-desktop-app.md),
refined by [ADR 0014](adr/0014-multi-workspace-redesign.md).

## How it runs (packaged)

The workspace gateway, its spawned engines, and embedded Postgres are a
**persistent background service**; the window is a client you open and close
(see § Always-on service + mobile access below for the *why*). The bundled Tauri
binary plays two roles, both in
`crates/lucidos-app/src/desktop.rs`:

**Service** (`Lucidos --service`, run by a launchd LaunchAgent — headless, no
window). On boot it:

1. resolves the OS app-data dir (survives updates) and the bundle `Resources`
   dir,
2. spawns the bundled `lucidos-gateway` on the **stable** port (default `5252`)
   with `LUCIDOS_GATEWAY_DATA`, `LUCIDOS_GATEWAY_PG_BACKEND=embedded`,
   `LUCIDOS_PG_BIN_DIR`, `LUCIDOS_PG_LIB_DIR`, `LUCIDOS_ENGINE_BIN`,
   `LUCIDOS_CLI_BIN` (the staged `lucidos` CLI the engine resolves for the
   coding-agent permission/question MCP servers, CC hooks, and chat-script
   `lucidos …` calls), `LUCIDOS_STATIC_DIR`, `LUCIDOS_SDK_DIR`,
   `LUCIDOS_SYSTEM_KNOWHOW_DIR` (the staged `system-knowhow/` reference docs —
   packaged builds have no source checkout for the repo-root fallback),
   `FASTEMBED_CACHE_DIR`, and `LUCIDOS_BOOT_WITHOUT_PROVIDER=1`,
3. the gateway creates/loads the workspace registry (first run finds it empty and
   creates no workspace — the smart root then serves the picker so the user names
   their first one), provisions embedded Postgres for workspaces that need it, and
   spawns one loopback-only `lucidos-engine` per running workspace,
4. supervises the gateway; the gateway supervises workspace engines and can
   re-adopt already-running engines after a gateway restart. On explicit
   `launchctl bootout` ("Quit and Stop Background Service"), the service tears down
   the gateway and every engine it spawned.

**Client** (the GUI app the user double-clicks). On launch it:

1. installs/updates `~/Library/LaunchAgents/com.lucidos.engine.plist` and
   bootstraps the service if it isn't already loaded (it starts at login via
   `RunAtLoad`),
2. installs `~/Library/LaunchAgents/com.lucidos.client.plist`, the **login
   agent**, so the client itself comes back at the next login too (below),
3. waits for the gateway health endpoint (`/~/api/v1/health`) on the stable
   port, then points the window at `http://localhost:<port>` (smart root: one
   workspace opens directly; multiple workspaces show the picker).

**Two agents, one per role.** `com.lucidos.engine` is the *service agent*: the
headless always-on stack, `RunAtLoad` + `KeepAlive`. `com.lucidos.client` is the
*login agent*: a one-shot `RunAtLoad` job whose whole body is
`/usr/bin/open -g -a <bundle> --args --login`, so the client is in the menu bar
after a restart instead of only after the user opens the app. Without it a
rebooted Mac has the engine running but nothing client-side: no menu-bar item,
no Dock badge, and no native notification banners, since those are shown by the
client process (`show_native_notification`) and not by the engine.

Three properties of that job are load-bearing:

- **`open`, not the bundle's inner binary.** On an already-running client
  LaunchServices activates that instance instead of starting a second one, so
  the job cannot produce a duplicate client no matter what kickstarts it. `-g`
  keeps the launch out of the foreground.
- **`--login` means menu-bar-only.** The `main` window is declared
  `"visible": false` in `tauri.conf.json` and shown in `setup` on every launch
  except this one, so a login start never flashes a window before hiding it. It
  comes up in exactly the state closing the window leaves the client in:
  tray item, no window, `Accessory` (no Dock icon). The hidden window still
  loads the gateway page, which is what keeps SSE, notifications and the unread
  count alive.
- **The user's "off" wins.** macOS lists the job under System Settings → General
  → Login Items ("Allow in the Background"). Switching it off records a launchd
  override keyed by the label, which the client's idempotent plist write never
  clears, and nothing in the code calls `launchctl enable`. The client only
  bootstraps the job when it actually (re)wrote the plist, which is a first
  install or a moved bundle.

No login agent is installed when the executable is not inside a `.app` (dev,
`cargo run`), and `--login` is ignored in dev, where there is no tray to reopen
a hidden window from.

Closing the window — red X, Cmd+W, or Cmd+Q — only dismisses the window; the
client stays resident in the macOS menu bar and the service keeps running
(triggers, scheduled tasks, coding-agent sessions, and mobile push keep going
headless). The only thing that stops the service is the explicit **Quit & Stop
Background Service** action — in the menu-bar (tray) menu and the app menu —
which `launchctl bootout`s it. "Open Lucidos" (menu bar) or a Dock click
re-shows the window.

The stable gateway port is persisted at `<app-data>/config/engine-port`
(historical file/env name; default `5252`; override with `LUCIDOS_ENGINE_PORT`)
so the mobile connect URL never changes across restarts. The gateway is the
network-facing surface; packaged engines bind loopback-only behind it.

`LUCIDOS_BOOT_WITHOUT_PROVIDER` lets workspace engines boot before any provider
is configured (they would otherwise panic). First run installs the
`UnconfiguredProvider` — a sentinel that boots cleanly but returns a clear
"No LLM provider configured" error on chat (never mock output) and reports
`llm_configured: false` on `/health`, which the app uses to show first-run
provider onboarding. The user adds a provider in **Settings → Providers**, then
restarts into the real provider.

None of this runs in development — `scripts/tauri-dev.sh` still uses Docker
Postgres + a natively-built engine, and the client launcher/updater
short-circuit on `tauri::is_dev()` (the service role is only ever started by
launchd in a packaged build).

## Building locally (unsigned)

```bash
cargo install tauri-cli --locked   # one-time
./scripts/build-dmg.sh
```

This builds the frontend, the release gateway + engine, fetches the relocatable
PostgreSQL 18 and compiles pgvector against it (the proven
`scripts/prototype/desktop-pg-pgvector-spike.sh` recipe), stages everything into
`crates/lucidos-app/bundle-resources/`, and runs `cargo tauri build --bundles
app,dmg`. The staging directory is gitignored and survives between runs, so
`cargo tauri build` verifies the staged `system-knowhow/` copy against the live
tree before it packages anything (`scripts/check-staged-knowhow.sh`, wired into
`beforeBuildCommand`). A build driven by hand rather than through this script
therefore cannot silently ship a months-old copy; it stops and prints the diff.
The result is an **unsigned** `.dmg` under `target/release/bundle/dmg/`
— Gatekeeper blocks it on other Macs (right-click → Open to run locally).

**The `.app` inside it is signed with the stable dev identity, when you have
one.** With no `APPLE_SIGNING_IDENTITY`, `build-dmg.sh` falls back to the
self-signed `Lucidos Dev Code Signing` certificate that
`./scripts/dev-codesign-setup.sh` creates (the same identity
`scripts/lib/codesign.sh` already applies to the dev engine binary), signing the
bundle inside-out exactly as the release path does. Without it, Tauri's ad-hoc
output gives the bundle a *cdhash-anchored* designated requirement, so every
rebuild is a new code identity and macOS re-prompts for, and discards, every
permission you granted the last build. The dev identity is a stable certificate,
so the requirement becomes `identifier "com.lucidos.app" and certificate leaf =
H"…"` and one Allow click sticks. Run the setup script once; until then the build
prints a hint and leaves the bundle ad-hoc, as before.

It is a *fallback only*, in both directions: an explicit `APPLE_SIGNING_IDENTITY`
always wins, and it can never carry a release. `--release*` asserts a Developer
ID up front, and the staging gate refuses a payload with no Team Identifier,
which a self-signed certificate never has. No hardened runtime and no secure
timestamp are applied for it, deliberately: both exist to satisfy notarization,
which this path never attempts, and the certificate-anchored requirement (the
entire point) needs neither. The DMG itself stays unsigned either way.

The lightweight packaging contract check runs without a macOS bundle build:

```bash
./scripts/build-dmg.sh --check
```

### Bundle `Info.plist` — keys Tauri's config can't express

Most of `Contents/Info.plist` is generated by the bundler from
`tauri.conf.json` (identifier, version, icon, `LSMinimumSystemVersion`,
copyright, category). For a key Tauri has no config field for, the bundler
**merges** a partial plist it finds *next to the config file* —
`crates/lucidos-app/Info.plist` — over the generated one. It is a normal plist
(XML doctype, one `<dict>`) holding only the extra keys; everything else keeps
coming from `tauri.conf.json`.

Today it carries exactly one key:

| Key | Value | Why |
|---|---|---|
| `NSUserNotificationAlertStyle` | `alert` | Requests the persistent **Alerts** notification style instead of macOS's default auto-dismissing **Banners**, so a native notification stays on screen until the user acts on it (they auto-dismiss after ~5 s otherwise). |

**It is a first-launch default request, not an override.** macOS reads the key
only when it creates the app's Notification Center entry; from then on the
user's own choice in System Settings → Notifications → Lucidos wins
permanently. So it changes what a *new* install starts with and never touches
an existing preference. Chrome's alerts helper and iMovie declare the same key.

Verify it survived a build (the merge is silent if the filename is wrong):

```bash
/usr/libexec/PlistBuddy -c "Print :NSUserNotificationAlertStyle" \
  target/release/bundle/macos/Lucidos.app/Contents/Info.plist   # -> alert
```

## Shipping (signed, notarized, auto-updating) — credentialed steps

These need an Apple Developer account, a Tauri signing key, and GitHub Releases.
They can't run from a CC worktree; do them on a Mac with the secrets present.

> **Automated by `scripts/release.sh` (host arch).** Cutting a release now builds
> the signed + notarized `.dmg` and uploads it — plus the auto-update artifacts
> (`.app.tar.gz`, `.app.tar.gz.sig`, and a generated `latest.json`) — to the
> GitHub Release it creates. The release **refuses to start** (before any
> force-push) unless all of `APPLE_SIGNING_IDENTITY`, `APPLE_ID`,
> `APPLE_PASSWORD`, `APPLE_TEAM_ID`, and `TAURI_SIGNING_PRIVATE_KEY` are set and
> `plugins.updater.pubkey` in `tauri.conf.json` is a real key (not the
> placeholder) — see `scripts/lib/release_signing.sh`. So the steps below are now
> mostly *setup* you do once (cert, updater key, pubkey); a release run performs
> the build + upload for you. The bundle version is stamped from the `RELEASE`
> file so artifacts are named `Lucidos_<version>_<arch>`. Coverage is the
> **host architecture only** — an Intel-only or Apple-Silicon-only `latest.json`;
> dual-arch is the CI follow-up below.

### Build-once / verify-first / publish-verified

`scripts/release.sh <version>` is the **one-shot** path: it goes public (force-push
mirror → tag → GitHub Release) AND builds + uploads the `.dmg` in a single
irreversible run, so you can't inspect the actual `.dmg` before it ships.

The two-phase flow splits that, so **both** halves of what ships are validated
before anything is published: the `.dmg` you mount is bit-for-bit the `.dmg`
users download, and the *commit* CI gated on is the very object that lands on
`main` (see [ADR 0024](adr/0024-the-release-candidate-is-the-published-artifact.md)).

```bash
# PHASE A — put the release candidate on the mirror, build + stage privately.
# Changelog is approved up front via -c; v<version> is NOT released here.
./scripts/release.sh -c <changelog-file> --verify-build <version> [<pr-number>]
#   → bumps RELEASE, splices the changelog, commits "Release v<version>";
#   → builds the STRIPPED release tree (scripts/lib/release_tree.sh: EXCLUDE_PATHS
#     dropped, WORKSPACES.md stubbed), SCANS it for private data (fail-closed),
#     commits it as a deterministic orphan and force-pushes it to
#     rc/<version> on the mirror — this fires the clean-machine source-install
#     gate, whose legs then run concurrently with the local build;
#   → records that rc commit as RC_COMMIT in the verify-build state;
#   → build → codesign → notarize → staple, and STAGES the artifacts (.dmg,
#     .app.tar.gz, .sig) + a manifest.json into
#     <worktree>/.lucidos/release-staging/<version>/;
#   → creates/refreshes the rc-<version> DRAFT RELEASE at rc/<version> with the
#     staged DMG + .sig, then DISPATCHES the clean-machine dmg-verify gate at it
#     (`-f dmg_tag=rc-<version>`). A draft is never listed on the public
#     releases page, and it fires no release event of its own, which is why the
#     dispatch is explicit (ADR 0036).
#     It STOPS here; the worktree + staging are left in place.

#   ⏸  Mount / install / launch / click around the staged DMG.
#   ⏸  Wait for all install-smoke.yml legs to go green on the rc.

# PHASE B — PROMOTE the validated candidate (no rebuild of the DMG or the tree).
./scripts/release.sh --publish-verified <version>
#   → promotion guard, refused BEFORE any public step: an RC_COMMIT must be
#     recorded, the mirror's rc/<version> must still point at exactly it (a moved
#     rc means CI's verdict belongs to a different commit), manifest.source_commit
#     must equal the worktree HEAD, and every staged sha256 must still match;
#   → force-pushes THAT SAME COMMIT OBJECT to the mirror's main → tags it there
#     BY SHA → creates the GA Release as a DRAFT → uploads the staged artifacts
#     (via build-dmg.sh --release-attach, which generates latest.json from the
#     staged .sig) → WAITS 25 to 45 min for release-tarballs.yml to attach the
#     four per-platform tarballs, which the tag push started → PUBLISHES the
#     draft → emits LucidosReleased →
#     LANDS the bump on local main (fast-forward, else cherry-pick, else a hard
#     failure — never a skipped bump) → tags THAT commit locally → pushes main +
#     the tag to `origin`, unforced → deletes the rc branch + the rc-<version>
#     draft release → cleans up the worktree + staging.
#     THE RELEASE IS INVISIBLE UNTIL IT IS COMPLETE (ADR 0042). Nothing is
#     public during that wait, so an interrupted one costs no rebuild:
#         ./scripts/release.sh --publish-draft <version>
#     Refuse-by-default if a tarball never arrives; --allow-missing-tarballs
#     publishes anyway, knowing `curl … | sh` will 404 for that platform.
#     The same tag name means a different object per remote, deliberately: the
#     mirror's names the orphan (the Release + download URLs resolve through
#     it), the local/origin one names the release commit on main. See ADR 0029.

# Re-arm the gate without a rebuild (rc push failed, or the rc was replaced):
./scripts/release.sh --push-rc <version>

# Finish a release whose draft exists but never went public (the wait above was
# interrupted). No push, no rebuild, no re-upload: wait, publish, emit, settle.
./scripts/release.sh --publish-draft <version>
```

Two guards make this safe, and both fail closed:

- **The staging manifest** (`scripts/lib/release_staging.sh`) — if anything in the
  worktree or the staged artifacts changed between the phases, Phase B refuses
  rather than shipping bytes you didn't verify.
- **The stripped tree + rc identity** (`scripts/lib/release_tree.sh`) — ONE
  definition of `EXCLUDE_PATHS`, the `WORKSPACES.md` stub, and the private-data
  scan, used by the rc push and the publisher alike, so the public rc branch can
  never carry internal content that `main` excludes. The rc commit is
  deterministic (identity + dates inherited from the release commit), so a
  retried or resumed Phase A re-pushes the identical object instead of
  invalidating the recorded SHA.

The build/upload split lives in `build-dmg.sh`: `--release-build` (build + stage,
no upload) and `--release-attach` (verify staging + upload, no rebuild); the kept
`--release` runs both back-to-back for the one-shot path (which has no rc gate).

### The updater payload is repacked from the SIGNED app (the v0.19.0 bug)

`cargo tauri build` packs `Lucidos.app.tar.gz` from the `.app` as the bundler
leaves it, and `build-dmg.sh` deliberately runs that build with
`APPLE_SIGNING_IDENTITY` removed from the **subprocess** env (Tauri's own
codesign pass skips the ~200 loose Mach-O files in the relocatable Postgres tree,
so the script signs the bundle itself, inside-out, afterwards). For the DMG this
is invisible, because `refresh_dmg_payload` re-injects the signed app into it.
Nothing did the same for the updater payload.

So from the introduction of the signed path until v0.19.0, **every published
`.app.tar.gz` contained an ad-hoc bundle**. The v0.19.0 payload, extracted from
the release: `Signature=adhoc`, `TeamIdentifier=not set`, designated requirement
`cdhash H"d3974ae45f…"`, no `Contents/_CodeSignature` at all. A cdhash-anchored
requirement changes with every build and macOS TCC keys permission grants on code
identity, so each auto-update silently destroyed every permission the user had
granted, and users who had only ever auto-updated were running an app that
`spctl` refuses to assess.

The fix has three parts, in `scripts/lib/updater_payload.sh`:

1. **Repack.** After `sign_app_bundle` and before `refresh_dmg_payload`, the
   tarball is rebuilt from the signed `.app` and the `.sig` is regenerated over
   the new bytes (a stale signature would make every updater reject the update).
   The signer is `tauri_signer_sign_file`, the single `cargo tauri signer sign`
   call site, shared with the release preflight's throwaway test-sign.
2. **Prove it round-trips.** The repack extracts what it just wrote and runs
   `codesign --verify --deep --strict` on the result, refusing to replace the
   tarball if that fails. It also enforces the layout
   `tauri-plugin-updater` needs: one top-level `.app` component (its extraction
   strips the first path component blindly), no hard-link entries (it resolves a
   hard link's target against the process CWD, not the extraction root, and
   `bsdtar` emits hard links where Tauri's own tar writer never did) and no
   AppleDouble `._` entries (they sit outside the `CodeResources` seal).
3. **Gate publication on it.** `stage_release_artifacts` and
   `upload_staged_assets` both extract the payload and assert its designated
   requirement is Developer ID anchored (`anchor apple generic` plus
   `certificate leaf[subject.OU]`) with a Team Identifier set. The verdict is
   re-derived from the bytes at each point rather than recorded in the staging
   manifest, so a staging dir produced by an older build, or a restamped
   manifest, cannot launder an unsigned payload into a signed-looking one.

The payload is packed **pre-staple**, which is the accepted cost already
documented under the deferred DMG below and is what keeps a deferred release and
an ordinary one producing the same artifact.

### Resumable notarization — Phase A survives losing the waiter

Apple's notary service regularly takes longer than the process waiting on it
lives, and the orchestration layer caps background tasks at 3600 s — so a slow
notarization can **never** be held in a foreground wait. The notarize stage
therefore never runs `notarytool submit … --wait`. It:

1. submits with `--no-wait` and reads the submission UUID immediately,
2. **persists a resume handle before any waiting**:
   `<repo-root>/.lucidos/release-state/notarize-<version>.json`, recording which
   of the two stages is outstanding, the submission id, the absolute path and
   sha256 of the file that was submitted, the source commit, the submit
   timestamp, and the updater payload the submission is paired with
   (`scripts/lib/release_notarize.sh`),
3. polls `notarytool info` until the status leaves `In Progress`,
4. staples (idempotently — re-stapling an already-stapled DMG is fine), stages,
   and then **drops the handle**, so a later run can't resume a finished release.

Losing the process at any point after step 2 therefore costs a poll, not a
rebuild. Before this, a killed waiter threw away a complete cargo release build,
~134 inside-out codesigns, and a signed DMG, because the staple, the staging dir,
the `manifest.json`, and the only copy of the submission id all died with it.

```bash
# Resume: skip build + codesign + submit; poll → staple → stage the existing DMG.
./scripts/release.sh --resume-notarize <version>
#   Re-running `--verify-build <version>` does this automatically when a handle
#   exists (and then no longer needs -c — that changelog is already committed).

# Adopt: a submission is in flight but its id was never persisted (it only existed
# in a log). Record it against the built DMG, then resume.
cd .lucidos/release-worktrees/<version>
./scripts/build-dmg.sh --release-build --adopt-submission <uuid> \
  --release-version <version> --staging-dir "$PWD/.lucidos/release-staging/<version>"
```

A resume is **refused** unless the DMG on disk still hashes to exactly what was
submitted *and* the tree is still on the commit it was built from. The second
check is load-bearing: the resuming run stamps `manifest.source_commit` from its
own HEAD, so resuming on a moved tree would make the manifest claim a commit the
DMG was never built from — and Phase B's identity guard would then pass on a lie.
Terminal cases are explicit: `Accepted` staples and stages; `Invalid`/`Rejected`
prints the notary log and refuses to stage; a submission id Apple doesn't
recognise says so and requires a fresh submit (it never silently re-submits).

Poll cadence knobs: `NOTARIZE_POLL_INTERVAL` (30 s), `NOTARIZE_POLL_TIMEOUT`
(7200 s — bounds only the current process; the handle outlives it),
`NOTARIZE_POLL_MAX_FAILURES` (5 consecutive transient errors).

### Deferred DMG — the release does not wait on Apple (ADR 0027)

Resumability keeps a slow verdict from costing a rebuild, but the *release*
still waited on it — 1 to 20 hours, every time. It never had to, because
notarization gates exactly **one** artifact:

| artifact | consumed by | Gatekeeper assessment? |
|---|---|---|
| headless tarball + `.sha256` | `curl … \| sh` | no — `curl` sets no `com.apple.quarantine` |
| `.app.tar.gz` + `.sig` + `latest.json` | in-app updater | no: the updater writes the bundle itself and sets no `com.apple.quarantine`, so Gatekeeper performs no assessment on launch; integrity is our minisign (Ed25519) key, which *is* checked |
| **`.dmg`** | browser download | **yes** |

Note (2026-08-02): the `.app.tar.gz` payload is currently ad-hoc signed rather
than Developer ID signed, tracked as F1 in
`docs/audits/2026-08-02-macos-update-path-audit.md` and being fixed in a separate
change. It is not what the middle row turns on, though: the absence of a
quarantine xattr is, so a signature on the payload is not the launch mechanism
here and should not be cited as one after F1 lands.

So a deferred release reaches every existing desktop user (auto-update) and
every terminal install (tarball) with no notarization involved. Only a
first-time Mac visitor downloading the DMG during the window is affected — and
lucidos.dev keeps serving the last notarized build, with a notice, until the
ticket lands.

```bash
# Phase A, without the notary wait: build → sign → submit → stage UNSTAPLED.
./scripts/release.sh -c <changelog> --verify-build --defer-notarization <version>
#   Against an ALREADY in-flight submission it stages without polling at all —
#   which is how a Phase A stuck on a slow verdict is rescued.

# Phase B: publishes with the "notarization pending" banner on the Release body,
# and KEEPS the worktree/staging/state/handle the attach step needs.
./scripts/release.sh --publish-verified <version>

# When Apple answers: staple → swap the asset in place → drop the banner →
# dispatch the clean-machine DMG gate → bump the site link → clean up.
./scripts/release.sh --attach-notarized <version>
```

The mode is **explicit and fail-closed**. Nothing falls back to it;
`--defer-notarization` is refused on `--release` / `--release-attach` (which
upload in the same process, where no banner can be composed); and the pending
state travels in the staging manifest's `notarized` field, so the banner, the
site link and the cleanup all read the same fact as the bytes. The `rc-<ver>`
draft release is deliberately **not** created for a deferred build, since `dmg-verify`
asserts a stapled ticket, so arming a gate that must fail says nothing. It runs
later against the published tag instead
(`gh workflow run install-smoke.yml -f dmg_tag=v<version>`), dispatched by the
attach step.

Two consequences worth knowing before using it. The updater tarball is built
**pre-staple**, so anyone who auto-updates during the window runs an unstapled
bundle permanently: invisible, since nothing assesses a non-quarantined bundle,
and re-issuing a stapled tarball could not reach them anyway. And an
`Invalid`/`Rejected` verdict now lands on an **already-public** asset: it can
never be notarized, so pull it with `gh release delete-asset`, leave the banner
up, and fix in a patch release. The attach step prints exactly that.

### 1. Apple Developer ID + notarization

Get a *Developer ID Application* certificate (Apple Developer Program), then:

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)"
export APPLE_ID="you@example.com"
export APPLE_PASSWORD="app-specific-password"   # appleid.apple.com → App-Specific Passwords
export APPLE_TEAM_ID="TEAMID"
./scripts/build-dmg.sh
```

`build-dmg.sh` explicitly signs and verifies the bundled `lucidos-gateway` and
`lucidos-engine` resource binaries and codesigns the `.app` with a hardened
runtime (`--deep` also signs the nested Postgres binaries/libs). It then makes
**two** notary submissions, in Apple's documented order: the `.app` is archived,
submitted and **stapled**, and only then is the DMG payload refreshed around the
stapled bundle, signed, submitted, polled and stapled in turn. Both are covered
by one resume handle (see "Resumable notarization" above for why submit and wait
are split). Without notarization, Gatekeeper blocks the download.

The app half was added on 2026-08-02 and is why a release now waits for two Apple
verdicts rather than one: `stapler staple` writes the ticket INTO the bundle, so
the copy inside the DMG can only carry one if the DMG is built around an
already-stapled app, and rewriting the image afterwards would void its own
signature and ticket. Before that, no shipped DMG contained a stapled app and a
DMG install's first launch had to reach Apple to be assessed. The cost, including
what it does to `--defer-notarization`, is recorded in ADR 0033.

An App Store Connect API key is used in preference to the Apple ID when
`APPLE_API_KEY_PATH` + `APPLE_API_KEY_ID` are set (plus `APPLE_API_ISSUER_ID`,
which is **required for Team keys and must be omitted for Individual keys**). The
Apple-ID fallback feeds `APPLE_PASSWORD` on **stdin** — never argv, which is
world-readable via `ps`, and never a `notarytool store-credentials` keychain
profile, which cannot work here: the release runs headless, so the Security
framework refuses the keychain write with "User interaction is not allowed".

### 2. Tauri updater signing key

```bash
cargo tauri signer generate -w ~/.tauri/lucidos-updater.key
```

Put the printed **public** key in `crates/lucidos-app/tauri.conf.json` →
`plugins.updater.pubkey` (replace `REPLACE_WITH_TAURI_SIGNER_PUBLIC_KEY`). At
build time, point the build at the private key file so the bundler signs the
update artifacts:

```bash
export TAURI_SIGNING_PRIVATE_KEY_PATH="$HOME/.tauri/lucidos-updater.key"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="…"
```

`TAURI_SIGNING_PRIVATE_KEY_PATH` is the self-documenting var holding the key
**file path**; the release scripts load its contents into
`TAURI_SIGNING_PRIVATE_KEY` (the only name Tauri's bundler reads). For
back-compat you can still set `TAURI_SIGNING_PRIVATE_KEY` directly to the key
contents (`"$(cat ~/.tauri/lucidos-updater.key)"`) or to a path Tauri
auto-detects — when `TAURI_SIGNING_PRIVATE_KEY_PATH` is unset that value is
honored unchanged.

This emits `*.app.tar.gz` + `*.app.tar.gz.sig` alongside the `.dmg`.
`bundle.createUpdaterArtifacts: true` is already set in `tauri.conf.json` (Tauri
v2 requires it for the macOS updater tarball), so you don't need to add it.

### 3. GitHub Releases + `latest.json`

`scripts/release.sh` does this automatically: it creates the Release tagged
`v<version>` on `github.com/lucidos-dev/lucidos` and uploads the `.dmg` (first
install), the `.app.tar.gz`, its `.sig`, and a generated `latest.json` (asset
uploads use `--clobber`, so re-running a release replaces them). The generated
manifest looks like this. `signature` is the verbatim contents of the
`.app.tar.gz.sig`, and there is exactly ONE platform key, describing the
**artifact**:

```json
{
  "version": "0.10.0",
  "notes": "What changed.",
  "pub_date": "2026-06-16T00:00:00Z",
  "platforms": {
    "darwin-aarch64": { "signature": "<contents of .app.tar.gz.sig>", "url": "https://github.com/lucidos-dev/lucidos/releases/download/v0.10.0/Lucidos.app.tar.gz" }
  }
}
```

That key used to be derived from `uname -m` at upload time, which describes the
machine running the upload rather than the payload (F10 in
`docs/audits/2026-08-02-macos-update-path-audit.md`), and a mislabelled key is a
**silent** failure: an updater whose target key is absent from `platforms`
reports "no update" rather than an error. It is now read off the staged app
binary with `lipo -archs` at BUILD time and recorded as `platform_key` in the
staging manifest, which is also the only shape that works for `--release-attach`
(that path deliberately has no `.app` on disk). `release_staging_verify` refuses
a manifest that records no key, so an old staging dir has to be re-staged rather
than guessed at. A universal binary is refused outright rather than emitting two
keys: the rest of the bundle is single-arch by construction (one relocatable
Postgres per target triple), so a second key would advertise an update whose
bundled Postgres is for the other architecture.

`plugins.updater.endpoints` already points at
`…/releases/latest/download/latest.json`, so a published Release is picked up by
the running client.

**How an update is surfaced + applied.** Detection lives in
`crates/lucidos-app/src/updater.rs` (three Tauri commands, packaged-only):
`check_app_update` (checks the endpoint, returns the new version or none),
`install_app_update_and_restart`, and `cancel_app_update`. The web app (running
inside the packaged Tauri client) polls `check_app_update` from **three
independent nets**: on every mount, hourly on an interval, and whenever the window
comes back to the foreground (throttled to one check per five minutes, since
`focus` fires on every window switch). All three exist because the client is
long-resident and rarely remounts: the window can be closed while it stays alive in
the menu bar, so a launch-only check misses a mid-session Release, and a client
left running with no resume net went on reporting itself current for the whole
interval after one (the 2026-07-31 case: a 0.18.0 client checked at launch while
0.18.0 still was the latest, then sat through 0.18.1 and 0.18.2). When an update
exists it shows an **in-app "Update & restart" toast inside the workspace**
(`crates/lucidos-app/src/store/actions/app-update.ts`). This is deliberate: most
users have a single workspace and auto-open straight into it, rarely seeing the
picker — so the message lives in the workspace, not the picker, and not a native
launch dialog (the old blocking dialog was removed). A plain browser / mobile PWA /
dev build shows nothing (they can't update the desktop app).

Clicking the toast runs `install_app_update_and_restart`, which restarts the WHOLE
stack onto the new version, not just the window: `Update::download` + `install`
(swap the bundle) → `desktop::restart_service()` (launchd `kickstart -k` → the service
supervisor tears down the gateway, engines, then embedded Postgres → launchd
respawns `--service` onto the NEW binaries → the fresh gateway brings the stopped
engines back, see below) → the client relaunch onto its new bytes. Order is
load-bearing — install first (new bytes on disk), then the service restart, then
the never-returning client relaunch. Without the service restart the window would
run new code against a still-old gateway/engine (the launchd service keeps the old
images until something restarts it).

**A restart brings back the workspaces it stopped** (`crates/lucidos-gateway/src/next_boot.rs`).
The service teardown stops every workspace engine and the embedded Postgres,
which is right for a full stop and wrong for a restart: the gateway that comes up
afterwards re-adopts only engines that survived (none did) and spawns only
`autostart` workspaces, so a workspace the user was sitting in stayed stopped,
and its open page could not wake it either, because API traffic deliberately
never lazy-starts a workspace (that guard is what makes the picker's Stop button
stick). On 2026-08-03 a packaged Restart left the open workspace down for
nine minutes until the page was reloaded by hand. So the teardown writes the ids it
stopped to `<app-data>/.next-boot.json` and `boot_all` consumes that record
(deleting it, so it is one-shot) and brings exactly those workspaces back
**regardless of `autostart`**: that flag governs the boot posture, not whether a
restart returns what it took. The same repair covers a gateway crash that launchd
respawns. **"Quit and Stop Background Service" writes `{"quit": true}` BEFORE it
calls `bootout`**, so the one teardown that means *stay down* records nothing;
declaring the intent first makes that ordering structural rather than a race
against how synchronous `bootout` is. A workspace stopped from the picker is not
running at teardown, so it never enters the record and Stop still sticks.

**The client relaunch goes through LaunchServices, so it comes back frontmost.**
`desktop::schedule_relaunch_after_exit()` spawns a detached watcher that waits
for this process to exit and then runs `/usr/bin/open -a <bundle>`; only if there
is no `.app` around us (dev, an unbundled binary) or the watcher cannot be
spawned does the path fall back to `app.restart()`. This is not decoration.
`app.restart()` fork/execs the new binary, which never asks the system to
activate it: the new instance can only land in front by inheriting the front slot
from its dying parent, and it loses that race whenever it registers with the
window server a moment too late. On 2026-08-03 the 0.20 → 0.20.1 update lost it
by ~280 ms, the front slot went to the next app, and the updated client sat
behind everything until the user Cmd+Tabbed to it. Launching *after* our exit
means there is no slot to inherit and no race to lose, and waiting for that exit
is also what keeps it to one instance (`open` against a live app activates it
rather than launching another; `open -n` would overlap two clients). The
**"Restart App"** action takes the same path.

**The install narrates itself, phase by phase.** All of that takes long enough —
a ~100 MB download, a signature check, a bundle swap, a service restart — that a
silent `await` reads as a frozen app, which is exactly how it behaved while the
plugin's progress callbacks were discarded. Every step now emits an
`app-update-progress` Tauri event carrying an `AppUpdatePhase` frame, and the page
turns it into a live toast (message + spinner + a determinate `.progress-bar`) and
mirrors it in **Settings → System**, which shares one derivation
(`appUpdateNarration`) so the two surfaces cannot disagree:

| Phase | Shown as | Cancellable |
|---|---|---|
| `checking` | Checking for updates… | yes |
| `downloading` | Downloading Lucidos `<v>` — 50 MB of 100 MB | yes |
| `verifying` | Verifying Lucidos `<v>`… | no (see below) |
| `installing` | Installing Lucidos `<v>`… | no |
| `restarting-services` | Restarting background services… | no |
| `relaunching` | Relaunching Lucidos… | no |
| `cancelled` | *(re-offers the update)* | — |
| `failed` | Update failed: `<reason>` | — |

Two properties are load-bearing rather than cosmetic. **Frames are throttled at
the source** — one per whole percentage point (or per MiB when the server sends no
`Content-Length`), always including the first chunk and the final byte count — so
a bundle that arrives as thousands of network chunks does not become thousands of
IPC messages. And **`total` may legitimately be absent**, in which case the UI
shows bytes with no percentage and no bar; fabricating one would be a lie about
progress we don't have.

`cancel_app_update` aborts the spawned download task. Only the check + download
can be cancelled: until the bytes are verified they exist solely in memory, so
abandoning them changes nothing on disk, whereas a half-swapped bundle has nowhere
to return to. The `AppUpdateRun` state machine in `updater.rs` enforces that
structurally (a run past `commit()` refuses cancellation) rather than relying on
timing. `verifying` is technically still abortable but withholds the button: it
lasts a few hundred milliseconds, and a control that appears and vanishes is
noise. The last phases also suppress ordinary toasts the way an engine restart
does — restarting the service kills the gateway serving the page, and the
resulting connection failures would otherwise bury the narration explaining them.

### 4. CI

There is no CI yet. A `tag → build → sign → notarize → publish` GitHub Actions
workflow (macOS runner, the secrets above) is the natural home for steps 1–3 so
releases are reproducible. Build the `x86_64` and `aarch64` bundles for full
coverage (`TARGET_TRIPLE` selects the relocatable PG; pass the matching Rust
target to the engine build).

## Always-on service + mobile access (implemented 2026-06-16)

> **Implemented** in `crates/lucidos-app/src/desktop.rs` (the service +
> LaunchAgent + stable-port lifecycle) and `crates/lucidos-app/src/mobile.rs`
> (connect URLs + Tailscale setup, surfaced in **Settings → Access**).
> This **supersedes** the window-coupled lifecycle the initial foundation
> shipped (`desktop.rs` used to boot the stack on launch and tear it down on
> `RunEvent::Exit` / `restart_app`). The runtime can only be fully verified by
> building a real `.app` on a Mac (launchd `bootstrap`/`kickstart`/`bootout`,
> the `--service` role, the `tailscale` CLI calls); the code `cargo check`s and
> the frontend type-checks + unit-tests clean.

**The gateway service is persistent; the UI is a client you open and close.**
Closing the window must NOT stop the gateway or workspace engines — triggers,
scheduled tasks, coding-agent sessions, and mobile push all have to keep running
with no window open (this is Lucidos's always-on event model; see CLAUDE.md §
Engine Statelessness).

- **Run the gateway service as a macOS launchd LaunchAgent.** The `.app` installs
  a plist into `~/Library/LaunchAgents/com.lucidos.engine.plist` on first run
  with `RunAtLoad` + `KeepAlive` (start at login, restart on crash, headless).
  The Tauri window and the mobile PWA are both pure clients of it. The client
  uses a **menu-bar (tray) model**: window close / Cmd+W / Cmd+Q hide the window,
  the client process stays resident to host the menu-bar item, and that item's
  **"Quit and Stop Background Service"** is the only teardown (`launchctl bootout`).
  Closing the window never stops the service.
- **Stable gateway port, not a random one.** The connect URL is stable across
  restarts. The packaged gateway owns the network-facing port; engines bind
  loopback-only behind it.
- **Show the connect URLs.** Surface localhost / LAN / Tailscale URLs (like the
  dev `show_banner`) so the user knows what to open on the phone.

**Mobile access = Tailscale (chosen), with the auto-setup reality:**

- **Mac side (scriptable after consent):** detect `tailscale`; if missing, guide
  the install (system VPN — needs user consent, can't be silent); run
  `tailscale up` (one-time tailnet login, or an auth key) then
  `tailscale serve --bg --https=443 http://127.0.0.1:<port>` for an auto-renewed
  HTTPS cert at `https://<machine>.<tailnet>.ts.net`. Full PWA + push, works
  off-LAN. (CLI 1.52 reworked that syntax; `system-knowhow/remote-access.md`
  § Route B carries both forms and which CLI takes which.)
- **Phone side (guided, not silent):** OS sandboxing prevents remote install/login
  — show a QR/link to install Tailscale and join the **same tailnet** (auth key
  can pre-authorize). Then open the `…ts.net` URL.
- **Use `serve`, not `funnel`.** The engine has **no inbound API auth**, so keep
  it tailnet-private (`serve`); do not expose it publicly (`funnel`) without first
  adding an inbound auth token. This is also why Tailscale is preferred over
  binding the raw LAN.
- **Fallbacks** when Tailscale isn't wanted: the mkcert local-CA route (LAN-only,
  README documents iOS trust) for PWA/push, or plain HTTP on LAN (browser only —
  no service worker / push, and unauthenticated LAN exposure).

## Packaged runtime environment (dev ≠ launchd) — audit fixes 2026-07-01

The packaged service runs under launchd with the bare system
`PATH=/usr/bin:/bin:/usr/sbin:/sbin` and none of the dev terminal's
environment. The 2026-07-01 DMG audit
(`docs/plans/2026-07-01-dmg-install-audit-fixes.md`) closed the gaps that
class of difference caused:

- **User-install PATH resolution.** The engine prepends the common
  user-install bin dirs (`/opt/homebrew/bin`, `/usr/local/bin`,
  `~/.local/bin`, `~/.npm-global/bin`) to its own process PATH at boot
  (`core::user_path::augment_process_path`, called from `main.rs` before the
  preflights), deduplicated and order-preserving, so every child —
  `claude`/`codex` fallbacks, `#!/usr/bin/env node` shims, chat bash/python
  tools, stdio MCP servers, Homebrew `git`, the `tailscale` CLI — resolves
  the same tools a dev shell would. On a dev PATH the dirs are already
  present, so it's a no-op by construction.
- **Agent binary resolution = override → probe → PATH.** `resolve_claude_binary`
  probes `~/.local/bin/claude`, `~/.claude/local/claude`, and the Homebrew
  prefixes (parity with codex) before the bare PATH lookup. The
  `coding_agent_claude_path` / `coding_agent_codex_path` preferences
  (Settings → Coding Agents) win outright when set, and an invalid
  configured path FAILS the spawn naming the setting.
  `GET /api/v1/coding-agents/binaries` reports the live per-agent resolution
  (override / detected / path / not-found) for that Settings section — only
  explicit overrides persist; detection is recomputed per request so a brew
  upgrade self-heals.
- **Bare `psql` works in coding-agent sessions.** `spawn_env::apply_lucidos_env`
  prepends `LUCIDOS_PG_BIN_DIR` (the bundled relocatable PG's `bin/`) to the
  subprocess PATH, matching what `workspace_script_env_vars` already did for
  chat/scheduled scripts — the advertised `psql -c '…'` contract holds in
  packaged CC/Codex threads.
- **Access shows only reachable URLs.** The "Local network" row derives
  from the configured gateway bind (`GET /api/v1/network-config`): loopback
  (the packaged default) shows guidance linking the Network access section
  instead of a dead `http://<lan-ip>` URL; `all` shows the detected LAN URL;
  a specific-IP bind shows that IP. Plain-HTTP rows carry the "no PWA/push"
  caveat — Tailscale (`serve`, https) remains the push-capable remote path.
- **Boot never waits on the embedding model.** Engine construction always boots
  with `memory::EmbedderSlot` empty (memory search/extraction/semantic thread
  search error descriptively until the model lands), and `spawn_embedder_load`
  loads the model in the background — trying immediately, then with capped
  backoff on a fetch-class failure (offline / HF blocked) — installing it +
  running the re-embed sweep without a restart. A healthy warm-cache boot is
  silent; the user is notified only after repeated download failure ("waiting on
  a download") and, if so notified, on recovery ("Memory is ready"). A corrupt
  model / config error stops retrying and disables memory with a loud
  notification, but never crashes boot. The model caches in
  `<app-data>/fastembed/`, which survives app updates — the download is once per
  machine, which is also why the model is NOT bundled into the DMG (it would
  ~triple every updater download for a file that never changes).
- **Smoke coverage.** `scripts/e2e-packaged.sh` now also asserts the
  notification/app-shell serving chain through the gateway proxy: non-stub
  `sdk.js`, `sw.js` served as JS with a stamped `BUILD_ID`, `manifest.json`,
  and the `push/vapid-key` endpoint.

## Status / remaining

The buildable foundation (launcher, standalone gateway handoff, engine
static-serving + mock fallback, updater wiring, bundle config, `build-dmg.sh`)
is in the tree and compiles. The signed build + `latest.json` + Release upload
are now wired into `scripts/release.sh` (host arch). What remains is
credentialed/Mac-only and tracked separately: the one-time updater keypair +
real `pubkey`, a first real notarized bundle build, dual-arch coverage, CI, and a
clean-machine first-run check.

Known follow-ups for the packaged build (surface them in the clean-machine pass):

- **Gateway-first packaged boot — wired (verify on a clean machine).**
  `build-dmg.sh` stages `lucidos-gateway` next to `lucidos-engine`, the launcher
  starts the gateway, and the gateway spawns engines by `LUCIDOS_ENGINE_BIN`.
  Confirm first-run behavior in a signed `.app`: an empty registry shows the
  picker (no auto-created workspace), and naming a workspace creates + opens it.
- **In-bundle JS SDK — wired (verify at runtime).** `/api/v1/sdk.js` (used by
  app-UI iframes) is now staged: `build-dmg.sh` copies `packages/lucidos-sdk/dist`
  to `<resources>/sdk`, the launcher sets `LUCIDOS_SDK_DIR`, and the engine's
  `find_sdk_bundle` checks it first. Confirm app UIs load the real SDK (not the
  warning stub) in the packaged build.
- **The primary "Restart" control** *(implemented)*. A workspace engine reports
  `packaged: true` from proxied `/api/v1/health`, and the frontend routes the
  Restart control accordingly: packaged + Tauri → `restart_service` (Tauri runs
  `launchctl kickstart -k` on the LaunchAgent), packaged browser/PWA →
  `POST /api/v1/restart` (the engine asks the gateway control API to respawn
  that workspace stack, with launchd as the legacy fallback), dev →
  `POST /api/v1/restart` (spawns `web-dev.sh --engine-only`). The supervisor
  catches SIGTERM and tears the gateway stack down gracefully before launchd
  respawns it. (The Tauri **"Restart App"** action restarts only the GUI client
  now — the gateway is the launchd service.)
