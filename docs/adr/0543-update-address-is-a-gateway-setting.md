# 0543: The update address is one gateway setting; a local rc channel tests an update before publishing

- **Status**: Accepted
- **Date**: 2026-10-21

## Context

A release candidate could not be tested through the in-app updater before it
was published. The packaged app's updater address was baked into
`tauri.conf.json`, and the gateway's release check (ADR 0108) polled a fixed
`lucidos.dev` route. Nothing read an override, so the first real update test
happened on users' machines.

## Decision

The **update address** is one machine-global gateway setting:
`[release_check] update_address` in `~/.lucidos/updates.toml`, beside
`enabled`. Its value is a `latest.json` URL, and empty means the default. The
gateway's hourly check and the packaged app's updater both read it at check
time. Settings, `lucidos update-address` and the control-plane
`PUT /~/api/v1/control/release-check` write it.

The **rc channel** is a folder in a dev workspace,
`data/artifacts/releases/rc/`, which its engine serves on loopback.
`scripts/publish-rc-channel.sh <version>` fills it from the release staging, by
hand only.

## Rationale

- **One place.** The gateway already owns the update check and its file, and
  both the app and the CLI already reach the gateway. An env var or a
  `defaults` key would need a relaunch and would hide from Settings.
- **A URL, not a folder.** The updater speaks `latest.json` over HTTP, so a URL
  needs no new serving code. The dev engine already serves workspace files on
  loopback.
- **The address moves where the app looks, never what it trusts.** The bundle
  is still checked with the baked `plugins.updater.pubkey`. A bundle signed by
  any other key is refused.
- **Fail closed and loud.** A refused or unreadable address is an error in
  Settings and in the check. It never falls back to the default, so a check
  can never read "up to date" against the wrong channel. While an address is
  set, Settings shows it with a one-click **Use default**.
- **The rule is narrow.** Only `https`, or `http` to `localhost`, `127.0.0.1`
  or `[::1]`, with no userinfo. `crates/lucidos-update-address` is the one
  definition, used by the gateway write path, the gateway poll and the app.

## Consequences

- With no address set, the gateway sends exactly the ADR 0108 request, and the
  app uses its configured endpoint.
- With one set, the gateway fetches it as a plain `latest.json` and sends none
  of the ADR 0108 query parameters.
- `dangerousInsecureTransportProtocol` is on in `tauri.conf.json`, so the
  plugin accepts a loopback http address in release builds. That switch also
  covers the bundle URL a `latest.json` names. So the app holds each offer's
  bundle URL to the same transport rule before any download. A test asserts
  every configured default endpoint stays https.
- A hostile `latest.json` can relabel an older signed bundle as newer, a
  downgrade. Whoever can set the address already holds full authority over the
  machine, since paired devices run coding agents with a shell. So the setting
  adds no new reach, and it needs no narrower authorization.
- A release that predates this setting has a baked address and cannot be
  pointed anywhere. The first testable hop starts from the first release that
  carries it.
- The rc channel serves only this machine. It is not a beta channel.

## Alternatives considered

- **An env var or a `defaults` key read by the app.** Lost: it needs a
  relaunch, the gateway would not see it, and nothing would show it in
  Settings.
- **A folder-valued setting, or a gateway route that serves staged files.**
  Lost: a new file server in the gateway, for what a dev engine already does.
- **Publishing the rc from Phase A automatically.** Lost: most releases skip a
  hand test, and copying a 100 MB bundle into a workspace on every release is
  waste. Phase A names the script in its handoff instead.
- **A public rc channel on lucidos.dev.** Lost: out of scope, and it would turn
  a local test aid into a release channel with its own guarantees.
