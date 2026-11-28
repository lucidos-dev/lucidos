---
name: Remote Access & HTTPS, The three routes
description: Part of Remote Access & HTTPS. Its index lists the sections it covers.
---

One part of `system-knowhow/remote-access`, which lists every part.

## The three routes

| Route | Setup cost | Browser trust | Secure context | Per-device work |
|---|---|---|---|---|
| **A. Plain HTTP over the tunnel** | none | "Not Secure" label | no | none |
| **B. `tailscale serve`** | one command, plus one account-level toggle | real Let's Encrypt cert, auto-renewed | yes | none |
| **C. mkcert** | generate a cert, point the gateway at it, restart | trusted only where the local CA is installed | yes | install **and trust** the root CA on every device |

A gateway bound beyond loopback on a tailnet that issues certificates needs
none of these: it serves HTTPS itself (§ Automatic, below). For a
loopback-bound gateway, recommend **B**. Fall back to **C** when the tailnet
HTTPS toggle is unavailable, and to **A** when the user does not care about
push or the label.

### Automatic: the gateway's own certificate

**A gateway bound beyond loopback serves HTTPS by itself** when the tailnet
issues certificates. It asks tailscaled for this machine's certificate, so
nobody installs anything and the user sees no setup step. This is a *managed
certificate* (ADR 0600). The gateway picks the first of these that works:

1. **Your own certificate**: `LUCIDOS_TLS_CERT`/`KEY` (Route C), when a phone
   would accept it for the tailnet name. It must cover that name, be in date,
   and chain to a public root (Mozilla's set). Nothing is fetched then.
2. **Your tailnet's**: when tailscaled reports `CertDomains` for this machine,
   the gateway fetches the certificate for the first name. It refreshes it
   daily and swaps it in with no restart. A failed refresh keeps the current
   one until it expires, but only while tailscaled still reports that name.
3. **None**: other devices stay on plain HTTP, and the gateway states why.

**Your own certificate and the tailnet's can serve side by side.** An mkcert
certificate chains to a private CA, and one made before a tailnet move names
the old address. Either way a phone refuses it for the tailnet name. The
gateway then serves each connection by the name the client asks for (SNI): the
tailnet name gets the tailnet certificate, every other name your own.
`lucidos tls status` says which certificate serves which name.

What the tailnet admin must enable (the agent cannot):

- **Tailscale**: <https://login.tailscale.com/admin/dns> → **HTTPS
  Certificates** → **Enable HTTPS**.
- **Headscale**: node certificates exist on `main` but remain unreleased as of
  2026-10-30. The admin must deploy a release that includes them, then enable
  `dns.certificates` and set `dns.challenge.provider`. Only then does the
  tailnet get automatic HTTPS.

Until then, a Headscale tailnet stays on plain HTTP. Source and headless operators can set
`LUCIDOS_TLS_CERT` and `LUCIDOS_TLS_KEY` instead (Route C).
The packaged app cannot take an operator certificate.

Things to know:

- **Only the tailnet addresses get HTTPS.** Loopback stays plain HTTP, so the
  desktop window and `tailscale serve` keep working. So do the LAN and other
  VPNs: their devices could not resolve the tailnet name anyway.
- **A gateway bound to a LAN address only gets no certificate.** The gateway
  asks for one only while it listens on a tailnet address of this machine, or
  on every address. Otherwise the HTTPS row says why, and no QR names a dead
  `https://` origin.
- **Old `http://` tailnet addresses still work.** Plain HTTP to a tailnet
  address gets a redirect to `https://<tailnet name>:<port>`. An old QR or
  bookmark lands on the secure origin.
- **Open the name, not the IP.** The certificate covers the tailnet name. A
  `100.x` address redirects to it.
- **Check it** with `lucidos tls status`, or the **HTTPS** row on Settings →
  Access. Both say which source serves, until when, and the reason when none.
- **Pairing follows it.** The pairing QR and Other addresses use the gateway's
  HTTPS origin, right after an external address.

### Route A: plain HTTP over the Tailscale tunnel

Zero setup. Open `http://mymac.tailnet-name.ts.net:5252/` (or the `100.x` IP)
from any device on the same tailnet.

The traffic **is** encrypted: WireGuard encrypts the tunnel end to end, and the
tailnet is private. So **confidentiality comes from the tunnel, not from TLS**,
and it is real. But the browser cannot see the tunnel. It applies the full
insecure-origin penalty: the "Not Secure" chip, no service worker, no push, no
clean PWA install. The UX cost is real even though the security worry is not.

Needs the gateway bound beyond loopback (see § Network bind, below).

### Route B: `tailscale serve` (recommended)

Tailscale terminates TLS with a **real Let's Encrypt certificate for the
MagicDNS name**, renewed automatically. Nothing to install or trust on any
device: phones, tablets and laptops just work, on or off the LAN. The gateway
can stay on loopback, because `serve` proxies from this machine to `127.0.0.1`.

**The agent cannot satisfy this prerequisite: Serve and tailnet HTTPS are
account-level, enabled in a browser by a tailnet admin.** The two layers fail
in ways that look nothing alike, so recognise both.

**1. `tailscale serve` prints a link and then blocks.** On a tailnet that has
never enabled Serve, the CLI does not fail. It prints this and polls the
control plane until someone visits the link (measured on CLI 1.96.4):

```
Serve is not enabled on your tailnet.
To enable, visit:

         https://login.tailscale.com/f/serve?node=<node id>
```

The node id is per-machine and cannot be reconstructed, so **that exact line is
the whole answer**. Open it and approve, and the still-running command finishes
by itself. From outside, `tailscale status --json` shows the same precondition
as an empty `CertDomains`.

**`--yes` does not help.** It suppresses interactive *prompts*, and this is not
a prompt: tested, the command blocks identically. Closing stdin does not help
either. Only the approval unblocks it.

**2. Cert provisioning fails outright** if Serve is enabled but tailnet HTTPS
certificates are not. Fix: <https://login.tailscale.com/admin/dns> → **HTTPS
Certificates** → **Enable HTTPS**.

```
500 Internal Server Error: your Tailscale account does not support getting TLS certs
```

Recognise that string at once. It is **not** a Lucidos fault, a network fault,
or transient. Retrying, reinstalling Tailscale, restarting the gateway or
regenerating anything fails the same way. Stop and ask the user to enable HTTPS
in the admin console. If they cannot (say, they are not an admin of that
tailnet), fall back to Route C.

Setup:

```bash
tailscale up                                             # once, interactive sign-in
tailscale serve --bg --https=443 http://127.0.0.1:5252   # front the gateway
tailscale serve status                                   # verify the mapping
tailscale cert mymac.tailnet-name.ts.net                 # provision/inspect the cert
```

The packaged desktop app does this for the user. **Settings → Access** runs
`tailscale serve --bg --https=443 http://127.0.0.1:<port>` behind its
**Expose** button. It waits out the tailnet approval above if needed, then shows
the resulting `https://mymac.tailnet-name.ts.net` URL. On the packaged app,
point the user there rather than hand-running commands. See § Settings → Access
for what the page can and cannot do, and § The Expose run for its steps.

**`serve` syntax changed in CLI 1.52, and the old form is now removed.** A given
CLI takes one of two forms:

| Form | CLI | On the wrong CLI |
|---|---|---|
| `serve --bg --https=443 <target>` | 1.52 and later | unrecognised flag |
| `serve https / <target>` | before 1.52 | `Error: the CLI for serve and funnel has changed`, exits non-zero, configures nothing |

`--bg` arrived with the 1.52 rework, so it belongs only on the first form. The
Expose button tries the current form, then the old one (`serve_arg_forms` in
`crates/lucidos-app/src/mobile.rs`). If both fail it reports **both** errors,
current first, with the retry labelled. One of them is always noise from the
form the CLI does not speak, so dropping either could drop the real reason.

**The fallback runs only when the CLI rejected a FLAG**, never on any other
failure. Otherwise a run that timed out awaiting tailnet approval would lead
with the legacy attempt's syntax error and bury the approval link.

**An attempt has TWO deadlines.** Since 1.52, `serve` without `--bg` holds a
foreground session until Ctrl-C. Before, it wrote persistent config. So a CLI
between that rework and the old syntax's removal runs `serve https / <target>`
in the foreground, and a hand-run of it never returns. An attempt gets 20
seconds to *configure* (a stall guard), then ten minutes once it prints the
approval link (a patience budget for a human). Output streams while the child
runs, so a killed attempt still reports everything it said, approval link
included.

Hand-running, `tailscale serve --help` is the authority for the installed
version, and `tailscale serve status` proves what got configured. A newer CLI
that rejects the old form **prints the exact command it wants**, so read the
error instead of guessing.

**Port 443 fronts exactly one target, so a second gateway needs a second
port.** Serve binds 443, 8443 or 10000. Lucidos probes all three, so Other
addresses and the pairing QR find the route wherever it is:

```bash
tailscale serve --bg --https=443  http://127.0.0.1:5252   # packaged install
tailscale serve --bg --https=8443 http://127.0.0.1:5251   # dev checkout
```

The phone then opens `https://mymac.tailnet-name.ts.net` and
`https://mymac.tailnet-name.ts.net:8443`. Both get the same certificate, since
it covers the DNS name, not the port.

A dev gateway that terminates its own TLS needs `https+insecure`, because
Tailscale has no reason to trust its certificate:

```bash
tailscale serve --bg --https=10000 https+insecure://127.0.0.1:5251
```

Serve re-terminates with the real certificate. That matters: an iOS home-screen
app keeps a pairing across relaunches on a trusted certificate and loses it on
an untrusted one.

**Do not path-prefix proxy** (`/dev/` → 5251) to fit two gateways on 443. It
breaks in a way that looks like a Lucidos bug. The gateway already owns the
first path segment as the workspace slug (`/<slug>/`, with `/~/` reserved for
the picker). Apps in the iframe assume they live at the origin root. An extra
prefix corrupts `<base href>`, app asset URLs and the SDK's `/api/v1` calls.
Use a second port instead.

### Route C: mkcert (Lucidos terminates TLS itself)

Use it when tailnet HTTPS is unavailable, for LAN-only access without
Tailscale, or on a dev checkout where the certs already exist.

**The certificate's SAN list must hold the MagicDNS name**, alongside
`localhost` and the tailnet IP. Generate it with every name a device might type:

```bash
brew install mkcert
mkcert -install                     # trust the local CA on THIS machine
mkdir -p .certs
mkcert -cert-file .certs/cert.pem -key-file .certs/key.pem \
  localhost 127.0.0.1 ::1 \
  "$(ipconfig getifaddr en0)" \
  "$(tailscale ip -4)" \
  "$(tailscale status --json | python3 -c "import sys,json;print(json.load(sys.stdin)['Self'].get('DNSName','').rstrip('.'))")"
```

If Tailscale is down, or `en0` has no address (wired-only, Wi-Fi off), a
substitution comes back **empty** and mkcert fails on the empty argument. Run
the substitutions alone first, then pass the values you got.

Verify before blaming anything else:

```bash
openssl x509 -in .certs/cert.pem -noout -text | grep -A1 'Subject Alternative Name'
```

A missing MagicDNS name shows on the phone as a name-mismatch error. It reads
like a trust failure and is not one, so check the SANs before walking anyone
through CA installation again.

Point Lucidos at the pair:

- **Dev checkout**: `.certs/cert.pem` + `.certs/key.pem` in the repo root are
  detected automatically and exported as `LUCIDOS_TLS_CERT` / `LUCIDOS_TLS_KEY`.
- **Headless install**:
  `./install.sh --tls-cert <cert.pem> --tls-key <key.pem>` writes them into the
  service environment. Both or neither: a lone flag is refused. A re-run
  without TLS flags, such as the update command in Settings, keeps the pair
  the service has; `--no-tls` drops it. Baked provider keys are kept the
  same way, and `--no-provider-keys` drops them.
- A Lucidos process serves https **iff both** variables point at readable files.
  TLS material is read at process start, so **restart the gateway** after a
  change; a live socket cannot be re-bound. See
  [Restarting the gateway](settings-access-3.md#restarting-the-gateway): Cmd-Q is not a restart.

**The critical caveat: mkcert signs with a local development CA.** Only
machines that trust that CA will connect, so every other device needs the root
installed *and* trusted:

1. `mkcert -CAROOT` prints the directory holding `rootCA.pem`.
2. Transfer `rootCA.pem` to the device (AirDrop is easiest for iOS).
3. Open it on iOS, then **iOS Settings → General → VPN & Device Management** and
   install the downloaded profile.
4. **Separately** go to **iOS Settings → General → About → Certificate Trust
   Settings** and toggle the mkcert root on.

**Step 4 is the one everybody misses.** The profile alone leaves the CA present
but untrusted, and Safari keeps rejecting the certificate. When a user says "I
installed the certificate but Safari still says it is not trusted", ask about
Certificate Trust Settings before regenerating anything.

Other mkcert gotchas:

- Restart Chrome after the first `mkcert -install`; it caches the CA store.
- Regenerate (and restart the gateway) whenever the LAN IP changes or the
  certificate expires, and repeat the trust steps on every new device.

### A second laptop: open it through its own Lucidos

**An SSH tunnel to `localhost` does not work.** The gateway serves
`https://localhost` with a *local certificate* that only its own machine
trusts, so the browser at the tunnel's far end refuses it. Two routes replace
the tunnel:

- **A remote gateway.** Install Lucidos on the laptop and pair the server's
  gateway in **Settings → Access → Remote gateways** (§ Pair a gateway).
- **The tailnet HTTPS address** (§ Automatic), when the laptop is on the
  tailnet.

## The local certificate: `https://localhost` on the gateway's machine

**Every browser on the gateway's own machine opens `https://localhost:<port>`.**
The gateway proves it is the real gateway with a *local certificate*, signed by
a certificate authority that belongs to this user alone. A process of another
user on the same machine cannot present it.

- **It needs trust once.** The desktop app shows a **Trust** screen before its
  window opens. A headless install trusts it during `install.sh` in a desktop
  session, and otherwise prints the command. Run `lucidos tls trust` any time.
- **macOS** keeps the trust in the login keychain and asks for the user's
  password. **Linux** adds it to the browsers' NSS databases with `certutil`
  (Chrome, Chromium and each Firefox profile).
- **Plain `http://localhost` is retired.** An old bookmark gets a page that
  carries unsent messages across as drafts, then opens the https address.
- **Every browser and phone pairs once more** after the upgrade, with a code.
  The desktop window re-pairs by itself, and paired gateways keep their
  pairing.
- **Wide plain listeners are unchanged.** A `--bind all` gateway without
  `LUCIDOS_TLS_CERT` still serves plain http beyond loopback, with today's
  pairing. Only loopback moved.
- **Check it** with `lucidos tls status`: its first line says whether this
  user trusts the local certificate.

## Any other VPN

**Lucidos only recognises Tailscale's address, because only Tailscale can tell
it.** WireGuard, ZeroTier, NetBird, a company VPN or a reverse proxy cannot
prove that a phone routes to them. So Lucidos never puts one in the QR on a
guess. You name it once, as the gateway's *external address*, and the pairing
QR uses it ahead of the tailnet (ADR 0550).

Set it in any of four ways:

- **Settings → Access → External address**, at the bottom of the page. The empty
  field shows the Tailscale address pairing falls back to. This machine's other
  addresses are links that fill the field, labelled by interface. A link only
  fills it: Save still checks it.
- `lucidos external-address set https://lucidos.example.com` in a terminal on
  the machine (`system-knowhow/lucidos-cli/subcommands-14.md` § `lucidos external-address`).
- `install.sh --external-address <origin>`, for a headless install that has no
  paired browser yet.
- `LUCIDOS_EXTERNAL_ADDRESS` in the gateway's environment. It wins and locks the
  other three.

What to know:

- **An origin only:** scheme, host and optional port, no path. Lucidos serves
  workspaces at `/<slug>/` and the picker at `/~/`, so a proxy must forward the
  whole host, not a sub-path. A loopback, link-local or unspecified address is
  refused, since no other device can open it.
- **One per gateway**, stored in `~/.lucidos/private/network.toml` (`~/.lucidos/network.toml` in a home an older release created) under the gateway's
  port. A second gateway on the same machine keeps its own.
- **Saving checks it.** The gateway fetches `<origin>/~/api/v1/health` and
  compares its per-boot `instance_id`. A match is verified. No answer from this
  machine saves with a warning, since hairpin NAT and client-only VPN routes
  are ordinary: test from the phone. An answer it cannot compare, such as an
  identity proxy's login page or a redirect, which it never follows, also saves
  with a warning. Only an answer from a different gateway refuses, because a
  code minted here does not work there.
- **The certificate is checked** against public roots. A company CA the probe
  does not trust reads as unreachable, with the reason, and still saves.
- **HTTPS on it** gives the installable app and push, as Route B and C do. A
  plain `http://` address pairs and works, without those two.
- **Pairing still guards it**, as on every route. For a public address, see
  § Public only behind a front door you own.

