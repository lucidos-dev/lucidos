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

A new feature needs no notice, because the changelog covers it. For a release
that does need one, follow these rules:

1. Give each change one notice item in `release-notices.toml`.
2. Use the real shipping version for `since`. Items with that version appear
   in one release notice.
3. Append in release order and keep every item ID stable. The workspace
   remembers which item it last answered.
4. Say what to do in a one-line summary of at most 100 characters. Put the
   explanation and links under Read more.
5. Use conditions and supersession to show only the items that still apply.
   Carry the replaced item's checks and action forward.
6. Name the smallest relevant checks in the audit sections that own them.
   Add a separate action sentence when the user needs something else.

The engine combines the shown items' requests into Check my workspace. Got it
or a successful check answers the items shown. Closing keeps them owed.
Adding a notice item is an ordinary change on `main` before the cut.

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

The blocker ledger lives in the workspace event store. The Software Factory app
writes it, and `scripts/lib/release_blockers_ledger.py` reads it through
`lucidos events count` and `lucidos events query`. A ledger is keyed by the
repository's root commit. Recording a blocker needs no repository commit.

The reader projects the records into one row per blocker, under the header
`id state ref work_item title reason`:

- `open` refuses every cut.
- `fixed`: `ref` is the fixing commit; passes only when that commit is in the cut.
- `withdrawn` passes. Only a paired device may withdraw.
- `waived`: `ref` is one `N.N.N` version. Only a paired device may waive.

A ledger counts only once a paired device opens it. A void, also device only,
removes a rejected record. An agent's device-only record is listed as
unconfirmed and changes nothing. Any doubt refuses with exit 1:

- no CLI, a CLI failure, or unreadable output;
- a count that moved during the read, twice;
- no device ledger, or two;
- an unvoided rejected record, an unknown `v`, or an `invalid` blocker.

An unattended release never writes a waiver. There is no waiver flag.

The digest is the 12-hex prefix of `git hash-object` over the snapshot. The
report names the ledger, its sequence and the digest. The local tag message adds
the whole snapshot, which is the record of what the gate judged.
`release_blockers_ledger.py --until-sequence` replays the snapshot at that
sequence as a cross-check. The sequence is the highest one the read saw, and
Postgres allots sequences before commit. So an emit in flight during the read
can land below it, and the replay then differs.

The record format and the app's script: the app's `knowhow/release-blockers.md`.

Phase A records its gate trailer in verify-build state for deterministic retries.
Phase B reads the ledger from the workspace event store again without rebuilding the candidate.
Waivers appear in the gate report, STILL OWED and the local annotated tag.

`ReleaseGateChecked` emission is best effort. It carries `ledger`, `ledger_sequence` and `ledger_digest`.
Nightly preflight returns SKIP on refusal, with a `release_blockers` JSON field.
For `auto`, a version-specific waiver refuses until the version is selected.
The mirror workflow checks the public trailer before either tarball push arm runs.

## Local rc builds

Use `scripts/release.sh --verify-build --rc <target>` for local testing (ADR 0727).
The target is the next `X.Y.Z` final. Each invocation durably allocates a fresh
`X.Y.Z-rc.N`; failures and abandonment never free a number.
These builds create no public branch, draft or release and run no public install-smoke gate.
Both plist version keys retain the full prerelease version.

Completed staging publishes automatically to Local, including completed
`--resume-notarize <version>` runs. A Local failure prints
`STILL OWED: scripts/publish-local-channel.sh <version>` without failing Phase A.
The publisher discovers its directory through the gateway CLI, verifies the
staged signature and hashes, and uses the staged manifest's publication time.
It serializes writes and refuses older offers or changed bytes under an existing version.
An identical retry succeeds. Never construct the feed or copy the bundle by hand.

Build the final with `scripts/release.sh -c <notes> --verify-build <target>`.
Without `--cut`, it uses the last rc's recorded cut, even if main moved.
An explicit differing cut lists untested and omitted commits before confirmation.
The final rebuilds every artifact because the version is compiled in.
Its own bytes pass every existing blocker, front-door and install-smoke gate before publication.
Beta only accepts these gated final bytes; it never carries local rc builds.

After public final publication, cleanup removes that target's rc worktrees,
branches, staging and state. It preserves allocation history, the Local feed,
other targets and any final inputs awaiting deferred completion.
Tester steps: [Test an update before publishing](desktop-app.md#test-an-update-before-publishing).

## Named update feeds

Beta and Local carry the packaged Apple Silicon macOS app. Any gateway can save
that machine-wide selection when the install inventory finds a desktop app.
Headless-only machines support Stable and Custom addresses. Nightly has no publisher.

Agents read the Local staging directory through their own engine (ADR 0729).
Channel writes print `request_id:` and exit 3 until the owner grants Allow once.
Ask the question tool with that id, then rerun the exact command. Omit `--port`
on this path. Local publication retains every staging hash and version check.

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
