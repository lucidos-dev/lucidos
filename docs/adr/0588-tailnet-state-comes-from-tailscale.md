# 0588: The tailnet address and MagicDNS name come from Tailscale's own status, LocalAPI first and CLI last, never from an interface name or address range; lucidos-tailscale may use serde_json

- **Status**: Accepted
- **Date**: 2026-10-27

## Context

The pairing QR, `lucidos pair --qr`, the Connect URLs and the bind hint all
read the tailnet address from `lucidos-tailscale`. It guessed from the interface
list: a `utun` or `tailscale*` interface holding an address in `100.64.0.0/10`.
A later fix added a second guess, preferring the interface that also held an
`fd7a:115c:a1e0::/48` address.

Both guesses held for one setup only: macOS, the stock control plane, IPv6 on.
They failed for:

- Headscale with its own IPv6 prefix, and IPv4-only tailnets;
- Linux and Windows interface names;
- a second VPN in `100.64.0.0/10` (Appgate, Zscaler, corporate CGNAT);
- no Tailscale at all beside such a VPN, whose address was shown as a tailnet.

The MagicDNS name came from a reverse lookup of the guessed address. A wrong
guess lost the name, and with it the `https://` serve URL.

## Decision

**Tailscale is asked.** `lucidos_tailscale::tailnet_state` sends
`GET /localapi/v0/status?peers=false` to the LocalAPI, found the way the
Tailscale CLI finds it:

| Platform | Endpoint |
|---|---|
| Linux | `/var/run/tailscale/tailscaled.sock`, then `/run/tailscale/tailscaled.sock` |
| macOS, open-source `tailscaled` | `/var/run/tailscaled.socket` |
| macOS, standalone app | `127.0.0.1:<port>` from the `/Library/Tailscale/ipnport` link, with the `sameuserproof-<port>` token |
| macOS, App Store app | the same TCP form, port and token from the proof file a bounded `lsof -c IPNExtension` lists |
| Any, Windows included | `tailscale status --json` through the CLI resolution, last |

The answer is one `TailnetState`: `Joined`, `NotJoined`, `NotRunning`,
`Unreadable` or `NotInstalled`. Each non-joined state has one sentence for the
user. From a `Running` status:

- the address is the first IPv4 in `Self.TailscaleIPs`;
- the MagicDNS name is `Self.DNSName` without its dot, only while
  `CurrentTailnet.MagicDNSEnabled`, with no suffix check;
- https is offered only when that name is in `CertDomains`.

The bind hint uses the address only when a UDP socket binds to exactly it.

**`lucidos-tailscale` may depend on `serde_json`.** Still never tokio, sqlx or
an engine crate.

## Rationale

Tailscale knows its own address and name. Every guess about them is a model of
Tailscale's internals that some setup breaks, and each fix added another guess.
Asking ends the series.

The CLI goes last because on macOS its shim runs the GUI executable. Outside a
GUI session that prints prose and exits 0. The LocalAPI has no such mode.

`serde_json` adds no crate to any binary: the gateway, engine, CLI and app all
build it already. The "libc only" rule existed to keep ADR 0014 §1 cheap, and
this keeps it cheap.

Any other VPN reaches a phone through the external address of ADR 0550, which
this change ships. Lucidos cannot prove a phone routes to an arbitrary VPN
interface, so it never puts one in the QR on a guess.

## Consequences

- An address shown or encoded anywhere is one Tailscale reported. When
  Tailscale cannot be asked there is none, and the UI and CLI say why.
- A Headscale tailnet keeps its MagicDNS name and never gets an https
  candidate, since Headscale issues no certificates.
- Every subprocess and socket read is bounded, so a wedged daemon costs a
  settings load a few seconds, never a hang.
- ADR 0587 holds for the names this now publishes. An https name brings fetch
  metadata, so the engine decides on `Sec-Fetch-Site`. A plain-HTTP name sends
  `Origin` equal to `Host`, which the gateway vouches for as it does for the IP.
- LAN detection still lives in the Tauri app, the gap ADR 0550 names.

## Alternatives considered

- **Keep the interface heuristic and add cases.** Each case fixed one setup and
  broke none of the others only by luck. The Appgate fix already needed the
  IPv6 prefix, which Headscale changes.
- **The CLI first.** It is what the app shipped before. It needs a `PATH` the
  packaged processes lack, and on macOS it answers with the GUI shim's prose.
- **Recognise other VPNs by interface name for the QR.** Rejected again for the
  reason ADR 0550 gives: a name proves nothing about what a phone can reach.
- **Read `serve-config` from the LocalAPI to decide https.** The end-to-end
  health probe proves more: that the URL reaches this gateway or workspace.
