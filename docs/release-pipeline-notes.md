# Release pipeline notes

Facts about building, signing and installing Lucidos that no ADR, script header
or workflow comment holds. The rule is `.claude/rules/build-release.md`.

## DMG layout without Finder

The v0.59.0 sandbox build failed when Tauri asked Finder to arrange the volume.
`build-dmg.sh` now asks Tauri for the app bundle only (ADR 0714).
`dmg_volume.sh` builds the image around the final app; `dmg_layout.py` writes
its five layout records, matching the shipped v0.58.0 window.
Never restore the Tauri DMG bundle or a CI-only layout bypass.
Run `bash scripts/lib/dmg_volume_test.sh` to check real image assembly and flags.

## Ask once per release: does it need a release notice?

The changelog says what changed. A release notice says what the user has to do
about it. It is the only surface that reaches the user unprompted, as a modal on
the first open after an upgrade.

Nothing derives a notice. A release that needed one and shipped without it tells
nobody, and the drift it caused sits silently in every workspace.

The test is narrow, and most releases correctly carry none. Write one when the
release leaves existing content or settings **stale rather than broken**, so
nothing errors and nobody notices.

- A renamed event that stops a trigger firing.
- A schema an app should migrate off.
- A setting that now wants configuring.

A new feature needs no notice, because the changelog covers it. Author the
notice in `release-notices.toml`, whose header owns the field rules, with `since`
set to the release in hand. Adding one is an ordinary change on `main` before the
cut, never a step inside `release.sh`.

## A bundled helper process carries its own entitlements

An entitlement belongs to one binary. The outer `.app` signature governs the app
process only. The engine and the gateway each need their own plist, applied to
that binary by name.

- `crates/lucidos-app/Entitlements.plist` covers the outer `.app` (the camera).
- `crates/lucidos-app/EngineEntitlements.plist` covers
  `Contents/Resources/lucidos-engine`.
- The engine embeds wasmtime and compiles a signer module, so it claims
  `com.apple.security.cs.allow-unsigned-executable-memory`.
- Release 0.32.0 shipped without that key, and macOS SIGKILLed the engine on
  every turn that reached a Wasm-signed proxy.

The gate runs the signed bytes. `sign_app_bundle` runs
`lucidos-engine --wasm-selftest` as its last step, after the outer codesign. A
readback says what the plist asked for. Only the run says what macOS allowed.
The difference is real: `allow-jit` reads fine and still crashes, because
wasmtime never asks for a `MAP_JIT` mapping.

Measure with a real Developer ID identity, never `-s -`. An ad-hoc signature does
not honour these keys the same way, and trusting it picked the wrong key first.
The static halves live in `build_dmg_test.sh`. See ADR 0160.

## Fetched payloads that reach a shell

Every place a fetched payload reaches a shell is sniffed first, and fails closed.
Production origins soft-404: they answer an unknown path with landing-page HTML
at status 200. `curl -fsSL` plus a non-empty test then passes, and the shell runs
HTML. There are five sites, all covered by `scripts/lib/install_test.sh`.

| site | reaches the shell via |
|---|---|
| `install.sh` `_source_libs` (helper libs) | `.` (source) |
| `install.sh` bootstrap guard (dash re-exec) | `exec bash -c` |
| `install.sh` `dispatch_uninstall` (fetched uninstaller) | `exec bash -c` |
| `uninstall.sh` `source_service_lib` (fetched `service.sh`) | `.` (source) |
| `uninstall.sh` bootstrap guard (dash re-exec) | `exec bash -c` |

- The last four assert that the payload starts with `#!`. `_source_libs` keeps a
  leading-`<` rejection, because its message names the lib it refuses.
- Both re-exec guards must stay POSIX sh, since they run before bash is
  guaranteed.
- `uninstall.sh`'s lib fetch is the likeliest to meet a soft-404. It derives its
  lib base from the origin (`${LUCIDOS_INSTALL_URL%/install.sh}/scripts/lib`),
  not from a checkout.

## Two installers, one `.app` layout

A machine can carry the `.app`, several `install.sh` instances and a source
checkout at once. Neither uninstaller can remove the other vehicle (ADR 0189).
Each one reports what it found and names the command that removes it.

The `.app` layout is written down twice. `crates/lucidos-installs` is the source
of truth. `service_desktop_app_paths` in `service.sh` mirrors its four constants
for the shell side. `service_test.sh` greps them out of the Rust and fails on
drift.

`install.sh` also says when it lands on the app's own port. The packaged gateway
cannot step aside, because the stable gateway port is an address that paired
devices hold. `report_coexisting_app` compares this instance's port with
`service_desktop_app_port`. Two ports get an info line. One shared port gets a
warning.

## Instance discovery is the `port` marker

`service_list_instance_names` lists exactly the `<prefix>/*/` directories that
carry a `port` marker. That file is the whole meaning of "installed".

- No marker means the instance is invisible to `--list`.
- No marker means `--all` never targets it.
- No marker makes `run_uninstall` return before the purge, which leaves the data
  directory and the shared runtime on disk.

Both launch shapes (service and foreground) write the marker.
The orderings stay deliberately different. `register_service` writes after its unit, so a failed registration leaves no marker. `launch_runtime` writes before an `exec` that never returns.

## Release blockers

`release-blockers.tsv` is private release metadata, stripped from the public tree.
Its tab-separated header is `id state ref work_item title reason`.
Use tabs between fields, end every row with a newline, and retain rows after they close.

- `open`: `ref` and `reason` are `-`; refuses every cut.
- `fixed`: `ref` is the fixing commit; passes only when that commit is in the cut.
- `withdrawn`: `ref` is `-`; a reason records why this was never a blocker.
- `waived`: `ref` is one `N.N.N` version; the owner must authorize the reason.

`work_item` is a tracker id or `-`. Neither the tracker nor event availability
decides the verdict. Commit ledger updates to main through the usual review.
An unattended release never writes a waiver. There is no waiver flag.

Phase A records its gate trailer in verify-build state for deterministic retries.
Phase B checks current main knowledge again without rebuilding the candidate.
Waivers appear in the gate report, STILL OWED and the local annotated tag.

`ReleaseGateChecked` emission is best effort; the ledger commit is the audit authority.
Nightly preflight returns SKIP on refusal, with a `release_blockers` JSON field.
For `auto`, a version-specific waiver refuses until the version is selected.
The mirror workflow checks the public trailer before either tarball push arm runs.

## Named update feeds

Beta and Local carry the packaged Apple Silicon macOS app. Headless gateways
support Stable and Custom addresses. Nightly has no publisher.

`release.sh --publish-beta <version>` verifies the recorded Phase A identity,
current release blockers, and every install-smoke run for that rc commit. Both
source-install and DMG-verification jobs must have succeeded. It reserves the
bundle hash before public mutation, moves only the scanned `channel/beta` tag,
and uploads immutable assets to the Beta prerelease. Retries compare existing
bytes. They never replace an asset under the same name.

`release-channel-versions.tsv` stays private and survives abandonment. A new
Beta supersedes the previous active candidate. Abandonment retires the active
version without reviving older candidates.

Cached candidate feeds live under
`.lucidos/release-channels/`; both lock files live there. The producer commits
only the private ledger before continuing, keeping the source checkout clean.
A missing cache rebuilds from the reserved version and immutable GitHub signature.
A corrupt cache fails closed. Never point Beta at unrelated bytes under an exposed version.

Stable remains the hosted feed's floor. Stable publication requests a refresh
after announcing the release. A failed refresh prints `STILL OWED` without
failing Stable. Retry with `release.sh --refresh-update-feeds <version>`.

The release scripts write `artifacts/update-feeds/beta/latest.json` through the
workspace CLI, then emit `UpdateFeedPublishRequested` with these fields:

```json
{
  "channel": "beta",
  "version": "1.2.3",
  "feed": "artifacts/update-feeds/beta/latest.json"
}
```

The workspace publisher deploys that file to
`https://updates.lucidos.dev/beta/latest.json` on the public Pages project.
It must preserve the JSON bytes and serve without Access or redirects. Deploy
from the maintainer's machine; no Cloudflare deployment credential goes to CI.
The producer HEAD-checks the bundle URL first, then verifies identical feed
bytes through both an edge read and a cache-defeating origin read.

Serialize publisher runs. Always read the current workspace artifact when a run
starts, rather than caching an older request's contents. A delayed request must
not overwrite a newer feed. Emit `UpdateFeedPublished` only after the current
artifact is served, with the channel, version and public URL actually verified.
