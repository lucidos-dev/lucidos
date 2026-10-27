# 0550: Reaching a gateway without Tailscale

- **Status**: Accepted
- **Date**: 2026-10-22

## Context

A new device pairs by scanning a QR that carries a *pairing address* plus the
code. `pairingOrigin` (`MobileAccessPage.tsx`) tries the tailnet first, then the
LAN address, then gives up and shows the code alone. Only the packaged desktop
app knows the LAN address: `detect_lan_ip` in `crates/lucidos-app/src/mobile.rs`
runs `ipconfig` on macOS. `lucidos pair --qr` knows the tailnet alone.

So without Tailscale a phone gets a LAN address at best, and only on the same
Wi-Fi. Nothing covers a device off that network. Nothing lets the user state an
address, and a headless install has no paired browser to state one from.

The motivating case is a company that dislikes Tailscale's cloud. It has no
enterprise agreement, and coordination and relay traffic can go through US
servers. The Tailscale app itself is allowed.

ADR 0094 already makes the gateway authenticate every network caller by
pairing, with no Tailscale in the auth path. So a new route is a transport
question, not an auth question. Wrong pairing guesses are rate limited
(`Redemption::Throttled` in `crates/lucidos-gateway/src/auth.rs`).

## Decision

**Two routes cover a device without Tailscale's cloud, and Lucidos runs neither.**

1. **A self-hosted Headscale tailnet** is a supported tailnet. The company runs
   Headscale and its embedded relay, and devices run the ordinary Tailscale app
   with `--login-server`. Lucidos stops assuming a tailnet name ends in
   `.ts.net`.
2. **An external address**: one origin per gateway, for a route the user runs.
   That covers a reverse proxy, a company VPN name, or a tailnet name with the
   company's own certificate. Lucidos advertises it and hosts nothing.

**Pairing stays the only gate Lucidos enforces**, on any route. For a publicly
reachable address, the docs recommend an outer gate the user owns: an identity
proxy, proxy auth or mTLS. They also say what a bare public address risks.

**The external address:**

- **Lives in the gateway's network config**, beside the bind. Pairing is per
  gateway (ADR 0132), and one proxy fronts the whole gateway.
- **Is an origin only**: scheme, host and optional port, no path. It passes the
  same check as a pairing QR origin (`pairing_qr::valid_origin`).
- **Is settable before any device is paired.** Four ways: an install flag
  (`--external-address`), an environment variable (`LUCIDOS_EXTERNAL_ADDRESS`),
  a `lucidos` CLI command on a running install, and Settings → Access. The
  environment variable wins and locks the Settings field.
- **Is probed on save** with the proof `verified_workspace_serve_url` already
  uses: health must name this workspace's `workspace_path`. Three verdicts:
  verified, unreachable from this machine, or reaches another gateway. The
  last is refused. Unreachable only warns, since hairpin NAT and client-only
  VPN routes are ordinary.

**The pairing address is the first of these that exists:**

1. the external address;
2. the tailnet origin (verified `serve` URL, else the MagicDNS name or tailnet
   address while something listens there);
3. the reader's own origin, when it is not loopback;
4. the detected LAN address.

With none, the code shows alone. The order has one definition, and the Access
page and `lucidos pair --qr` both take it. Connect URLs lists each source as
its own row.

**The engine detects the LAN address**, as it detects the tailnet address. It
reports the private IPv4 on the interface carrying the default route, on every
OS, in `GET /api/v1/network-config`. The Tauri copy is deleted. Lucidos does not
guess at other VPN interfaces.

**A tailnet name is proven by equality, not by suffix.** A reader is on the
tailnet when its hostname equals the MagicDNS name the engine resolved for its
own tailnet address. That holds for `*.ts.net` and for a Headscale domain alike.

## Rationale

**The user's own infrastructure is the only route that scales without us.** A
work laptop's corporate VPN routes into the employer's network, not to a home
machine. So off the home network the choice is public reachability, a tailnet,
or a relay. Lucidos can describe the first two without owning a service.

**Pairing already carries the weight a public address puts on it.** ADR 0094
designed the gateway to refuse every unpaired caller and to trust no peer
address. Guessing is throttled. A second gate of our own would be new security
code with its own failure modes. A mature identity proxy in front is a better
outer gate, and the user's to choose.

**An address the user typed is explicit intent, so it wins the QR.** Someone
who wants the tailnet in the QR on a machine with both clears the setting. The
reader's own non-loopback origin is free proof: the page arrived through it, so
it reaches this gateway.

**A headless install must be able to pair its first device.** No browser can
reach Settings before a device is paired, so the address must be settable from
the install and the shell. A container also reports a pod IP as its LAN
address, which is why the external address outranks detection.

**An interface name does not say who can route to it.** A recognizer per VPN
product repeats the trap Tailscale-specific code already is, and still proves
nothing about the client side. The external address covers those routes
honestly.

**Headscale keeps most of Lucidos unchanged.** The client, its interface and its
address range are Tailscale's own, so tailnet detection, the plain-HTTP URL and
the pairing QR already work. Only the `.ts.net` assumption and `serve` HTTPS
differ.

## Consequences

- `remote-access.md` § "Still do not put Lucidos on the open internet" changes
  in the implementing change: public only behind a front door the user owns,
  with pairing as the inner gate.
- `tailscale serve` HTTPS needs Tailscale's certificate service, so it does not
  work on a Headscale tailnet. HTTPS there means a company certificate (Route
  C) plus the external address set to that `https://` name.
- A Headscale tailnet with a custom address range outside `100.64.0.0/10` is not
  detected as a tailnet (`lucidos_tailscale::tailnet_ipv4`). Its user sets the
  external address instead.
- A probe that cannot run is unknown, never a refusal. Only a positive "reaches
  another gateway" answer refuses.
- The README install contract gains `--external-address` beside `--bind` and
  `--tls-cert`.
- The glossary entries *external address* and *pairing address* land with this
  ADR, marked not shipped. The implementing change drops the marker.
- As shipped, the stored value is keyed by gateway port: an `[external_address]`
  table in `network.toml`. That file is machine-global, and two gateways can
  share a machine, so one shared value would put one gateway's address in the
  other's QR. The environment variable is per process, so it needs no key.

## Alternatives considered

- **A relay Lucidos runs.** Best ergonomics, but we would operate a service
  that carries every workspace's traffic. It also needs a lucidos.dev origin
  and credentials (ADR 0031), which self-hosting is meant to avoid.
- **Private routes only.** This keeps "never public". But it rules out the one
  route that helps a work laptop off the home network. Lucidos cannot tell a
  public address from a private one anyway.
- **Our own outer gate** before pairing on non-private addresses. More code
  in the security path, duplicating what an identity proxy does better.
- **Only a verified address feeds the QR.** Locks out exactly the setups where
  the host cannot reach its own public name.
- **No probe.** A typo, or an address aimed at the other gateway, silently
  ships a QR carrying a code that gateway never minted.
- **An origin plus a path.** Every workspace URL, the `<base href>`, the
  picker's `/~/` and the cookie path would need a prefix-aware rewrite.
- **A per-workspace preference.** Two workspaces would disagree about one
  proxy, and the pairing QR is a gateway concern.
- **Tailnet first in the order.** A user with both could never put the proxy
  in the QR.
- **An address picker on Add a device.** Adds a choice to a one-tap flow.
- **Listing every interface.** Prints Docker bridges and dead addresses, the
  bug the Access page was built around.
- **Recognizing NetBird, ZeroTier or WireGuard interfaces.** One recognizer
  per product, still without proof of reach. NetBird is a different client, so
  it reaches Lucidos through the external address.
