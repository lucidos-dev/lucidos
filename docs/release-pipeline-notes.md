# Release pipeline notes

Facts about building, signing and installing Lucidos that no ADR, script header
or workflow comment holds. The rule is `.claude/rules/build-release.md`.

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
