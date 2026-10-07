---
name: Remote Access & HTTPS
description: Use when the user wants to reach Lucidos from a phone, tablet or another machine. Covers "remote access", "Mobile Access", "Expose", "tailscale serve", "tailscale funnel", "expose a webhook". Also "HTTPS", "not secure warning", "add to home screen", "certificate", "mkcert".
---

# Remote Access & HTTPS

Reaching a Lucidos install from a phone, a tablet, or a second computer. Two
knobs decide whether it works, and they are **independent**:

1. **Where the gateway listens** (the *network bind*): loopback only by default,
   so nothing off this machine can connect directly.
2. **Whether the browser sees a secure origin** (https, or localhost): this
   decides the "Not Secure" label, and whether service workers, web push and
   PWA install work at all.

Most confusion comes from mixing them up. A perfect tunnel still shows "Not
Secure" over `http://`. A perfect certificate is useless if the gateway binds
loopback and nothing proxies to it.

## Diagnose first, never assume

**A machine often runs more than one Lucidos gateway, with different TLS
setups.** The usual pair is the packaged `Lucidos.app` on **5252** (plain HTTP
by default) and a dev gateway from a source checkout on **5251**. The dev one
serves **https** whenever the checkout has `.certs/cert.pem` + `.certs/key.pem`.
Advice for one is wrong for the other, so probe before you say anything.

### Step 1: who is listening, and on what address

```bash
lsof -nP -iTCP -sTCP:LISTEN | grep -i lucidos
```

Read **both** columns that matter:

| Bind shown | Meaning |
|---|---|
| `127.0.0.1:5252` (or `[::1]:...`) | loopback only. No other device can connect directly. `tailscale serve` still can, because it proxies from *this* machine. |
| `*:5251` | all interfaces. Reachable on the LAN IP and the tailnet `100.x` IP. |
| `100.x.y.z:5251` | bound to the tailnet address specifically. |

Expect one `lucidos-gateway` row per install, plus one `lucidos-engine` row per
running workspace. **Every engine should read `127.0.0.1`**, packaged and dev
alike. The gateway is the only network-facing surface, because it alone
authenticates callers. An engine row on `*:` or a `100.x` address means
something set `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`: that workspace is reachable
with no credential. Hand a remote device the **gateway** port, which routes to
every workspace by slug.

### Step 2: which port speaks TLS

```bash
for p in 5251 5252; do
  printf 'port %s  https=%s  http=%s\n' "$p" \
    "$(curl -sk -o /dev/null -w '%{http_code}' --max-time 3 https://127.0.0.1:$p/)" \
    "$(curl -s  -o /dev/null -w '%{http_code}' --max-time 3 http://127.0.0.1:$p/)"
done
```

| Reading | Conclusion |
|---|---|
| `https=307`, `http=000` | that port terminates TLS |
| `http=307`, `https=000` | that port is plain HTTP |
| `307` on either | **healthy.** The gateway redirects `/` into a workspace path (`/<slug>/`) or the picker (`/~/`). A 307 is the normal answer, not an error. |
| `200` | also fine (a direct workspace or picker URL) |
| `000` on both | nothing listening there, or a firewall ate it |

`-k` is deliberate: it asks "is TLS spoken here" without mixing in "is the
certificate trusted". Those questions have separate fixes. Check trust
separately, by name, from the device's point of view:

```bash
curl -sv https://mymac.tailnet-name.ts.net/ 2>&1 | grep -i 'subject\|issuer\|SSL certificate'
```

### Step 3: repeat from the remote device's viewpoint

Probe the **MagicDNS name**, not `127.0.0.1`. A cert can be valid for
`localhost` and useless for `mymac.tailnet-name.ts.net`, and a loopback probe
never shows that.

## Devices pair before Lucidos answers them

**The gateway authenticates every caller that reaches it over the network.** An
unpaired device gets a pairing screen, at the address it asked for. So reaching
the port no longer makes you the user.

**The desktop app pairs itself.** Its Rust side reads the machine-local token,
which a browser cannot, so it mints a code and spends it without asking. That
makes it the first device on a fresh install, with no terminal.

Any paired device can then let the next one in. **Settings → Access → Add a
device** mints a code and draws a QR the phone can scan.

A terminal does the same, and is the fallback when nothing is paired at all:

```bash
lucidos pair            # prints a code; type it into the device
lucidos pair --qr       # draws it as a QR the phone can scan
lucidos pair --port 5251  # when two gateways are running, say which one
```

It finds the gateway by probing. With two running it refuses to guess and names
the ports it found, since a code only works on the gateway that minted it.

`lucidos` is on no `PATH`. A desktop install keeps it at
`Lucidos.app/Contents/Resources/lucidos`. A headless install keeps it under the
install prefix, in `runtime/current/`.

The code works once and expires in five minutes. A device stays paired until
you revoke it: there is no idle or absolute timeout. An expiry would cut off
only the devices you forgot, since a credential in use never goes stale.

**Devices says when it last saw each one**, to the nearest day. That tells a
phone in daily use from a laptop you sold, so read it before you revoke. The
browser cookie has its own window, refreshed each day the device is seen, so an
active device never reaches it. The gateway never reads that window: age is not
an input to the auth decision.

Five things here surprise people:

- **A device pairs to a GATEWAY, not to a machine.** This shows only on a
  machine running two gateways, which is supported (packaged on 5252, dev on
  5251). Each keeps its own device list, codes and cookie. So one refuses a code
  the other minted, and Settings → Devices lists the gateway serving that page.
  Pair the phone once per gateway: the pairings sit side by side and do not
  evict each other, even on one hostname. ADR 0132 says why the local token
  stays shared while the device list does not.
- **A browser pairs even on that machine.** Proving you are local means reading
  a file only your user can read, and a browser cannot read files. So Safari on
  the host pairs like a phone. The CLI and the desktop app's Rust side never
  pair, because both attach that proof themselves.
- **Being on the tailnet is not what authorizes you.** Auth reads no Tailscale
  header and works the same over `tailscale serve`, mkcert or a plain LAN
  address. The tailnet is transport.
- **A workspace's own engine port is not a way in.** Engines bind loopback, so
  only this machine reaches one. Every other device pairs and goes through the
  gateway at `/<slug>/`. A bookmark straight at an engine port stops resolving
  from elsewhere, on purpose: it bypassed pairing.
- **Apps still act with your authority**, though no longer with your realm. An
  app frame runs at an opaque origin and reaches the engine only through the
  host. The host forwards a named set of `/api/v1` sub-trees and stamps your
  device on each. So an app cannot read the shell, its storage or another app,
  or copy the credential off the machine. Within those sub-trees it still calls
  the API as you.

### Still do not put Lucidos on the open internet

Authentication raises the floor. It does not make a public origin a good idea.

- Use `tailscale serve` (tailnet-private), **never `tailscale funnel`** on the
  gateway's own port.
- No router port-forward, no public reverse proxy, no ngrok-style tunnel.
- Keep the tailnet as the outer boundary, with pairing as the inner one.

### The one exception: a webhook's own socket

A *webhook* is the one surface meant for callers who will never join your
tailnet: GitHub cannot pair. So webhook deliveries answer on their own port, the
*hook socket*. That port is the only thing you point `tailscale funnel` at (ADR
0097).

**The isolation is structural.** Funnel maps a *port*, never a path, so it
cannot express "expose only the webhooks". The hook socket has one route,
`POST /<slug>/<webhook-id>`, and answers 404 to everything else, a wrong method
included. So a public caller reaches no control plane and no workspace. Funnel
on the gateway's own port would put both one auth bug away from the internet.

The hook port is the gateway's plus ten: **5261** in dev, **5262** packaged.
`LUCIDOS_HOOK_PORT` overrides it, and `0` switches the socket off.

```bash
tailscale funnel --bg 5262   # packaged: publish ONLY the hook socket
tailscale funnel status      # what is public right now
tailscale funnel off         # stop publishing
```

`tailscale funnel --help` is the authority for the installed version, as
`serve --help` is below. Deliveries arrive at
`https://<machine>.<tailnet>.ts.net/<slug>/<webhook-id>`.

**A public port is not an open door.** Every delivery still authenticates, by
bearer token or by the sender's own signature. Each webhook emits one pinned
event, fixed at creation. Create one with `lucidos webhooks create`.
`system-knowhow/lucidos-cli.md` covers the tokens and the GitHub, Slack and
Stripe signature shapes.

**Senders resend.** GitHub retries a slow response, Stripe retries for days,
and by default each arrival emits again. `--dedupe` names the header carrying
the sender's delivery id and collapses the repeat. The CLI page covers it, and
the `--headers` allow-list that puts a chosen request header in the payload.

**Lucidos watches this path, and only watches.** Every 15 minutes it knocks on
each public webhook address from outside and expects a 401. It reads only
`tailscale serve status --json` (to learn the funnel's port), and never runs a
mutating `tailscale` command, so it never re-arms a funnel you turned off.

It catches one address family going dark while the other still works, which
one probe alone would miss. The outage shows as a bar across the app and a line
on the row in **Settings > Webhooks**. It emits `WebhookIngressDegraded` and
`WebhookIngressRecovered` once each per outage, for a *trigger* to act on.

**A sleeping computer is not a dead funnel.** A round the machine slept
through, or the first one after a wake, never counts toward an outage. A sleep
of 30 minutes or more emits `WebhookDeliveriesSleptThrough` once, with
`slept_secs`. GitHub does not resend a failed delivery on its own, so a trigger
on it can tell you to check the sender's delivery log. Never answer it by
re-arming the funnel: nothing failed there.

## Why HTTPS matters, and when it does not

Browsers gate some features on a *secure context* (https, or anything on
`localhost`):

- **Service workers**, and with them **web push notifications** and reliable
  **PWA install / standalone launch**.
- Clipboard API, Notification API, geolocation, media capture.

Plain `http://` to a tailnet name is not a secure context. All of the above are
off, and Safari and Chrome show a "Not Secure" label. A user who only reads and
chats from a laptop, without push, can choose that. A phone that behaves like an
app needs one of the two TLS routes.

## The three routes

| Route | Setup cost | Browser trust | Secure context | Per-device work |
|---|---|---|---|---|
| **A. Plain HTTP over the tunnel** | none | "Not Secure" label | no | none |
| **B. `tailscale serve`** | one command, plus one account-level toggle | real Let's Encrypt cert, auto-renewed | yes | none |
| **C. mkcert** | generate a cert, point the gateway at it, restart | trusted only where the local CA is installed | yes | install **and trust** the root CA on every device |

Default recommendation: **B**, then **C** when the tailnet HTTPS toggle is
unavailable, then **A** when the user explicitly does not care about push or the
label.

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
port.** Serve binds 443, 8443 or 10000. Lucidos probes all three, so Connect
URLs and the pairing QR find the route wherever it is:

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
  service environment. Both or neither: a lone flag is refused.
- A Lucidos process serves https **iff both** variables point at readable files.
  TLS material is read at process start, so **restart the gateway** after a
  change; a live socket cannot be re-bound.

**The critical caveat: mkcert signs with a local development CA.** Only
machines that trust that CA will connect, so every other device needs the root
installed *and* trusted:

1. `mkcert -CAROOT` prints the directory holding `rootCA.pem`.
2. Transfer `rootCA.pem` to the device (AirDrop is easiest for iOS).
3. Open it on iOS, then **Settings → General → VPN & Device Management** and
   install the downloaded profile.
4. **Separately** go to **Settings → General → About → Certificate Trust
   Settings** and toggle the mkcert root on.

**Step 4 is the one everybody misses.** The profile alone leaves the CA present
but untrusted, and Safari keeps rejecting the certificate. When a user says "I
installed the certificate but Safari still says it is not trusted", ask about
Certificate Trust Settings before regenerating anything.

Other mkcert gotchas:

- Restart Chrome after the first `mkcert -install`; it caches the CA store.
- Regenerate (and restart the gateway) whenever the LAN IP changes or the
  certificate expires, and repeat the trust steps on every new device.

### Fourth option, when the other device has a terminal

`ssh -L 5252:localhost:5252 <host>` then `http://localhost:5252` gives the full
app including push, with no certificate at all, because **localhost is a secure
origin**. Useless on a phone, ideal for a second laptop.

## Settings → Access

The page that drives all of this: point the user here before hand-running
commands. It was called **Mobile Access** until the **Network access** bind
setting moved onto its bottom from Settings → System.

**It answers two independent questions, numbered in setup order**, because the
first must be true before the second buys anything:

| Section | Question | Where the answer comes from |
|---|---|---|
| 1. The machine running Lucidos | Is the engine's machine on a tailnet? | `detected_tailscale_ip` from `GET /api/v1/network-config` in any browser; the fuller `get_connect_info` probe on the packaged desktop app |
| 2. This device | Has the device reading the page joined that tailnet? | The address this device was served on |

**Add a device is the action they lead to**, not a third question: pairing a
new device at one of those addresses. See its own section below.

Every section renders **everywhere**, phone browsers and the installed PWA
included, Connect URLs among them. Platform changes how much each can say,
never whether it appears. Only the **actions** are gated. Sign in to Tailscale
and Expose are native commands with no HTTP equivalent, so only the packaged
desktop app has them. **Get Tailscale** is a link, not a bridge call, so it
shows wherever it can be acted on. It opens the App Store on iOS, the Play
Store on Android, and `tailscale.com/download` otherwise.

Do not restore the old shape, where the page showed **one** section chosen by
platform. A gateway bound to its tailnet address serves remote devices at a
bare `100.x` host. A browser there saw only section 2, and was told to install
the Tailscale it was already using.

**Section 2 reads the device it runs on**, so it never asks a device to redo a
done step. A web page cannot inspect a phone's interfaces, so the evidence is
the host it was served on:

| How this device got here | The page shows |
|---|---|
| Loopback (`localhost`, `127.0.0.0/8`, `::1`, `*.localhost`) | "You are reading this on the machine that runs Lucidos". No install offer and no app-install step: this device IS the machine, so section 1 is its whole answer |
| A `*.ts.net` name | "Tailscale is connected on this device" |
| The exact `100.x` address section 1 reports for the machine | the same, for the same reason |
| Any other address | **Get Tailscale**, then all three steps (install Tailscale, copy the machine's Tailscale address, install the app) |

**Being on the tailnet is not the last question.** The remaining step also
depends on whether the origin is secure (`window.isSecureContext`). The
installable app and push need one, and Route A's `http://` tailnet address is
not one:

| On the tailnet, and… | The remaining step |
|---|---|
| a secure origin (`https`, or loopback), in a browser tab | install the app here |
| plain `http://`, in a browser tab | **none, on this device.** The page says so and points at `tailscale serve` on the machine, because the browser will offer no install control here however the page words it |
| already the installed app | nothing |

Each proof is sound. A MagicDNS name resolves only on a device signed in to
its tailnet. A request to the machine's tailnet address arrived over that
tailnet. The engine read that address off a Tailscale **interface**:
`lucidos_tailscale::tailnet_ipv4` requires the interface *and* the range. A
loopback request never left the machine.

A bare `100.64/10` host that does **not** match the reported address is not
proof. That range is real CGNAT space an ISP can hand to a physical interface.
An unproven host keeps the install offer, the harmless way to be wrong.

**The page never calls a device a phone unless it is one.** Desktop browsers
read it as often as handsets, and have no home screen. So the "add to home
screen" wording sits behind an iOS/Android check, and the neutral phrasing is
"install Lucidos".

**Connect URLs** lists the addresses that reach **this workspace**:

- **This Mac**: `http://localhost:<port>/<slug>/`. A secure origin, so a full
  PWA works here (Route D). Packaged desktop app only, since the localhost port
  comes from the Tauri bridge.
- **Local network**: only when the gateway is bound beyond loopback. Plain HTTP,
  so no PWA install and no push. Under a loopback bind (the packaged default),
  the row points at the **Network access** section further down this page
  instead of printing a dead URL. Packaged desktop app only, since detecting a
  LAN address needs the bridge.
- **Tailscale**: the tailnet address over plain HTTP until `serve` is verified,
  and `https://<name>.ts.net/<slug>/` once it is. Rendered **everywhere**,
  including a phone browser: see § The tailnet-status endpoint.

**Every row carries the `/<slug>/` prefix**, because that addresses a workspace
(ADR 0014). A bare origin 307-redirects to the sole workspace or the picker,
the wrong address on an install with several workspaces.

Both plain-HTTP rows obey the network bind: being on a tailnet does not mean the
gateway **listens** on the tailnet address. Under the packaged loopback default
neither prints, because both URLs would be dead. A bind pinned to the tailnet
address shows the Tailscale row and reports the LAN as off. `serve` needs no
wider bind, so the **HTTPS row is never bind-gated** and survives the packaged
default.

The bind that counts belongs to whichever process served the page. Behind the
gateway that is `gateway_bind`. On a direct engine port the origin is the
engine, which follows the gateway only while `[engine] inherit` is on.

### Add a device

It sits under Connect URLs, as what you DO with one of those addresses. It
mints a *pairing code* and draws it as a QR, so a phone scans rather than
typing eight digits. The reader is already paired, which makes the offer safe:
a paired device holds full authority and may enrol another.

**A live code shows as three cards**, one per way to use it: scan the QR (the
big card), type the digits, or open the address. The cards are alternatives, so
each after the first says "Or". The digits are always there, large enough to
read across a desk, so a failed scan is never a dead end. The two fallbacks
carry a Copy button, hidden where the browser has no clipboard (a plain-HTTP
LAN address is not a secure context). The address card is text, never a link:
following it here would spend a single-use code on a device already paired.

**The QR encodes `<reachable-origin>/~/?pair=<code>`.** Scanning it opens the
picker, whose pairing screen reads the parameter and fills in the code. It then
strips the code from the address bar, so a reload, bookmark or shared link
cannot keep it. On a phone the scan lands somewhere else: see § A phone
installs before it pairs.

**The address is the hard part.** The machine minting a code usually reads this
page over loopback, and a QR aimed at `127.0.0.1` helps nobody. So the section
uses the same order as the Tailscale row. First the verified `serve` origin,
else the MagicDNS name, else the tailnet address, each only while something
listens on it. Then the LAN address, on the packaged desktop app. With none of
those it mints the code and says there is no QR.

**The expiry is a live countdown**, above the cards beside the New code button.
When a code expires the cards go, leaving the countdown's verdict and the
button. **Nothing here mints a code by itself.** A phone back from installing
may find its code expired, and the reader presses the button for another.
Auto-replacing it read as the page undoing the reader's own press (ADR 0098).

The section needs `/~/…` to reach the gateway, which holds exactly while the
page is served under `/<slug>/`. Served straight off an engine port, that path
gets a 404 from the engine, so the section says to run `lucidos pair` instead.
It never hides, because its heading is a Search Everywhere destination.

### A phone installs before it pairs

**On iOS the home-screen app is a different device from Safari.** It has its
own storage container, so the credential cookie taken in a Safari tab never
reaches it. iOS also cannot route a scanned link into an installed web app: the
Camera app always hands it to Safari. So pairing the tab enrols the wrong thing
and leaves the app locked out. Android has no such problem, because an
installed PWA captures links inside its own scope.

So the pairing screen shows a phone browser the **install steps**, not the code
form. Add Lucidos to the home screen and open it. The code rode into the
manifest's `start_url`, and the app spends it on sight with nothing typed. To
pair the browser anyway, tap **Pair this browser instead**.

**That code is fixed at install time, and still lasts five minutes.** A slower
install opens on the pairing screen with the code refused, and recovers by the
routes below. A fresh code on the host does not reach it, because the app's
launch URL was written at install.

**The app spends that code once and then ignores it.** iOS relaunches from the
stored launch URL, so the code returns on every cold start. Retrying it would
fail, and each failure spends part of the gateway's wrong-guess budget for the
minute. After the first attempt, the app treats the launch as carrying no code.

**An app already on the home screen cannot be reached that way**, since its
launch URL is fixed. The pairing screen inside the app offers three ways
across:

- **From the browser on the same phone.** It is a separate device with its own
  credential, so it is often still paired when the app is not. Open the same
  address there and mint a code under Settings → Access. The screen says so
  when it runs as an installed app.
- **Paste code.** The browser screen offers Copy, and the pasteboard is shared.
- **Scan QR.** The app opens its own camera and reads the QR off the host's
  screen. It shows on a phone over HTTPS, where a camera can open at all. An
  expired code leaves no QR, so the host makes a fresh one first.

**A paired app that suddenly asks again has lost its cookie, not its row.** The
credential lives only in the container's cookie jar, and an iOS home-screen app
can drop that while keeping everything else. So Lucidos hands the app a fresh
copy on every page load. If one is lost anyway, the device still shows under
Settings → Devices. Pairing again adds a second row rather than repairing the
first, so revoke the older one.

Typing the eight digits still works everywhere, and is what a desktop browser
does.

### The list of devices lives in Settings → Devices

Access adds a device. **Settings → Devices** lists every device and holds
**Revoke**. One row per device carries both actions:

| Action | Reach | What it does |
|---|---|---|
| **Revoke** | the whole machine | Stops this device reaching Lucidos, on every workspace. |
| **Remove** | this workspace | Forgets its push subscription and its preferences here, and leaves it paired. |

Both halves key on the id the gateway minted at pairing, so each device has one
row. (There used to be separate **Paired devices** and **Devices** lists.)

A row can lack either half, and neither is an error. A device paired from
another workspace holds nothing here yet, and its row says **Not set up in this
workspace**. It keeps a push toggle, off and disabled, because push hangs off
the engine row it lacks. A browser on a direct engine port never went through
the gateway, so it has nothing to revoke. With no gateway at all the pairing
column is dropped, and Search withholds the **Revoke** hit on that page.

The row states the present, never a history it cannot see. Nothing there claims
a device has never opened this workspace, because a missing engine row does not
prove that. **Remove** deletes the row of a device sitting right in front of
you. A device that paired before its two ids were unified keeps them apart
until it next loads the page.

A device used on one day and never again drops off the list by itself after a
week. Most are automated browser runs, since every fresh browser profile
arrives as a new device. A device someone named, paired, or turned push on for
never drops off this way. See *one-off device* in the glossary.

### What a device is called

Each half carries its own name, and the row prefers the one you can edit.

The pairing screen suggests a name read off the browser, such as `Chrome on
Mac` or `Safari on iPhone`. The person at the device may overwrite it, and that
typed name wins: the holder is more specific than whoever minted the code. So
`lucidos pair --label` is a fallback for an empty field, and the CLI says so
when it prints the code. An unrecognised browser suggests nothing and leaves
the field blank, which keeps the fallback reachable. With neither, the device
is listed as "Paired device".

That *pairing label* is fixed: revoke and pair again to change it. The name on
the **Devices** row is not. Click it and type, and the row shows that from then
on.

**One rule names a device everywhere**: the typed name, else the pairing label,
else the browser and machine from its user-agent plus the start of its id, as
in "Chrome on Mac (109371a3)". Failing all three, it is `device-` plus the
first eight characters of its id. The Devices row, a message's Origin popover,
an actor chip and the agent's list of your devices all follow it. The gateway
passes the pairing label to the workspace on every request. A device reached
straight on an engine port has no pairing label and skips that step.

A message's Origin shows the device's current name, so a rename reaches older
messages too. The device you read on is marked "(this device)". The whole id
never shows: it is unreadable, and at that length it wraps.

### The tailnet-status endpoint

`GET /api/v1/tailnet-status` puts the Tailscale row in a browser. It returns
two fields, each a string or null:

| Field | Meaning |
|---|---|
| `magic_dns_name` | `<machine>.<tailnet>.ts.net`, no scheme. Null off a tailnet, and null with MagicDNS turned off |
| `workspace_serve_url` | The `https://<name>/<slug>/` URL, published only once verified |

Only the machine can run the reverse lookup behind the name, so a browser has
no other way to learn it. The plain-HTTP row prefers the name over the bare
`100.x` address: it resolves anywhere on the tailnet, and a person can retype
it on another device.

**`workspace_serve_url` is verified end to end, never inferred from a
listener.** With two gateways (443 fronting 5252, 8443 fronting 5251), a live
443 can belong to one that has never heard of this slug. So the engine fetches
the candidate URL's own `api/v1/health`, with TLS validated, and compares the
reported `workspace_path` with its own. A same-named workspace on the other
gateway lives at a different path, so a match is proof. The probe costs nothing
off a tailnet, and both halves are bounded.

It is a separate route from `network-config` on purpose: the bind editor
fetches that one too, and must not pay for a reverse lookup and a round trip.
The packaged app's own `serve_url` (from `get_connect_info`) answers "is this
MACHINE serving", for the Expose row. This endpoint answers "what is the URL for
THIS workspace".

**Tailnet state is read without the Tailscale CLI.** The page takes the tailnet
address from the machine's interface list and the MagicDNS name from a reverse
lookup. So it reports correctly however Tailscale was installed (App Store,
standalone app or Homebrew), even in a packaged process with no `PATH`. When
debugging:

- A machine with **MagicDNS disabled** has a tailnet address and no name. It is
  still on the tailnet, with no HTTPS name to serve.
- A CLI is needed for **`tailscale serve` only** (and the Sign in button).
  `serve` has no GUI, config-file or admin-console equivalent. So a Mac without
  a CLI can be described but not exposed. The page says so and names two ways
  to get one: Install CLI in the Tailscale app, or `brew install tailscale`.

**The four states of section 1 on the packaged desktop app** come from those
two independent facts. A browser shows only the tailnet half, with no action,
because every action below is a native command:

| Tailnet state | CLI | The page shows |
|---|---|---|
| Tailscale absent | any | **Get Tailscale** |
| Installed, not on a tailnet | yes | **Sign in**, with an optional auth key |
| Installed, not on a tailnet | no | Sign in from the Tailscale menu-bar app |
| On a tailnet, not serving | yes | **Expose** |
| On a tailnet, not serving | no | How to get the CLI; the plain-HTTP URL works meanwhile |
| On a tailnet, serving | any | The `https://...ts.net` URL, plus **Re-apply** with a CLI |

**A failed Sign in or Expose shows the underlying error verbatim.** The toast
adds no Lucidos framing, because every message the page raises already names
what failed. That is the missing CLI, the missing tailnet address or MagicDNS
name, `tailscale <cmd> failed: <stderr>`, or a post-condition that reported
success and changed nothing. So read the toast as the CLI's own words. For a
syntax rejection the CLI prints the exact command it wants, and that line
**is** the fix.

One line is filtered out: `Warning: client version "..." != tailscaled server
version "..."`. Tailscale prints it on stderr for *every* command when CLI and
daemon versions differ. That is the normal state of a Homebrew CLI beside the
Mac app's daemon. With output streamed, it would otherwise lead every error.
Still resolve it before debugging `serve` in earnest (see § Operational notes),
though it is not the error.

### The Expose run

Pressing **Expose** starts a supervised run, not one blocking call. It narrates
each step in a toast, and the **brand badge** in the header spins for the whole
run on any screen. Close the toast and the run carries on: tap the Lucidos
mark, and the run's line in the menu unfolds to the current step and its
actions. Every step is indeterminate, so both spin rather than show a bar.

| Step | What is happening |
|---|---|
| Setting up Tailscale access | Locating a CLI, then reading the tailnet address and MagicDNS name |
| Configuring tailscale serve | The `serve` command is running |
| Waiting for you to enable Serve on your tailnet | The CLI printed an approval link (see Route B). The toast offers it as **Enable in Tailscale**; approve it in the browser and the run continues on its own |
| Waiting for HTTPS to come up | The mapping is written; polling 443 for up to 30s, because a first-run certificate takes a moment |

The run ends as the `https://...ts.net` address, an error shown verbatim, or
nothing if cancelled. **Cancel** is offered throughout and leaves no serve
config behind (killing the child before it commits writes nothing). Only one
run exists at a time: pressing Expose during one is refused. The button reads
"Setting up…" and stays disabled even if you navigate away and back, because
the run lives in the app, not the page.

If someone reports "Expose did nothing", the run may be waiting for them on
Tailscale's site, and the badge says so. A frozen window is a different bug:
every Mobile Access command runs on a worker thread, never the UI thread.

**Never run `/Applications/Tailscale.app/Contents/MacOS/Tailscale`.** It is the
GUI executable, not a CLI. Outside a GUI session it prints "The Tailscale GUI
failed to start ... (Tailscale.CLIError error 3)" and **exits 0**. So an
exit-code check reads success with unparseable output. That once made Mobile
Access show a Sign in button that reported success and changed nothing. Use
`/usr/local/bin/tailscale` or `/opt/homebrew/bin/tailscale`.

## The detection trap: Tailscale state does not tell you whether HTTPS works

If the gateway terminates TLS itself (Route C), **Tailscale is not in the
request path at all**. Then:

- `tailscale serve status` prints `No serve config`.
- `tailscale cert` may be entirely unprovisioned.
- And `https://mymac.tailnet-name.ts.net:5252/` works perfectly.

An agent that checks only Tailscale state concludes "HTTPS is not set up". It
then re-solves a solved problem, or worse, runs `tailscale serve` and shadows a
working setup with a half-configured one.

The inverse trap: a `serve` mapping that *exists* does not prove it works,
because the certificate may never have provisioned (the account toggle above).

The Access page applies this rule with two different probes:

- The **Expose / Serving row** (packaged desktop app, from `get_connect_info`)
  claims serving only once **port 443 on the tailnet address** has a listener.
  That proves a listener, not a working certificate, so the row can show while
  a first-run cert still provisions.
- The **Connect URLs Tailscale row** needs the candidate URL's own
  `api/v1/health` to answer over validated TLS (§ The tailnet-status endpoint).
  Until then it shows the plain-HTTP tailnet address.

Neither asks `tailscale serve status`. That answers "does *a* serve mapping
exist", a different question. With only the two-gateway setup's 8443 mapping,
the config is non-empty while `https://<name>` on 443 is dead. Each surface
tests exactly the endpoint it is about to name.

**Rule: the port is the source of truth.** Probe it directly before any claim
about HTTPS:

```bash
curl -sk -o /dev/null -w '%{http_code}\n' https://mymac.tailnet-name.ts.net:5252/
```

## Network bind

Direct access (Routes A and C, and any LAN or tailnet-IP URL) needs the gateway
bound beyond loopback. `tailscale serve` does not, since it proxies locally.

- Machine-global config lives in `~/.lucidos/network.toml`:
  `[gateway] bind = "loopback" | "all" | "<IP>"` plus `[engine] inherit`.
- **`[engine] inherit` reaches a directly-launched engine, not one the gateway
  spawned.** A gateway-spawned engine is pinned to loopback whatever the file
  says, since the gateway is the only door that authenticates.
- Edit it from the **workspace picker → Settings → Network access** (the
  gateway bind), or per workspace in **Settings → Access → Network access** (the
  engine bind, when `inherit` is off).
- `./install.sh --bind all` writes the same file.
- Default is **loopback**, and a malformed value fails safe to loopback, never
  to all interfaces.
- A change takes effect only after a **restart**.

### An engine that faces a network asks for a credential

Widening the **gateway** bind changes nothing about how you reach Lucidos. The
gateway authenticates every caller on every bind, by pairing or by the
machine-local token. Your phone pairs once and keeps working.

Widening an **engine** bind is different, and only a directly-launched engine
can be widened. It then requires a local credential on every path but
`/api/v1/health` (ADR 0155). A browser cannot present one, so:

- **Reach the workspace through the gateway**, at `https://<host>:<port>/<slug>/`.
  That is the shipped route, and it is unaffected.
- **A bookmark straight at an engine port stops working from another device.**
  On the machine itself it works only for processes that can read
  `~/.lucidos/local-token`, such as the `lucidos` CLI.
- Local callers need no setup. The CLI, the gateway's proxy hop and the webhook
  hop already present their credential.

Two credentials exist, both minted by the gateway, both mode 0600.
`~/.lucidos/local-token` reaches everything, and `~/.lucidos/webhook-token`
reaches only webhook delivery. So a `tailscale funnel` on the hook port cannot
restart a workspace.

Binding to the tailnet IP itself (`100.x.y.z`) is the middle ground: the
tailnet reaches it, the coffee-shop LAN does not.

**A configured IP that is not up yet does not hold the start back.** An `<IP>`
bind always comes with loopback, and only loopback is required. The gateway
serves on loopback at once, retries the configured address in the background,
and listens there once the interface appears. This matters at boot: launchd
starts the service before `tailscaled` assigns the `100.x` address, so that bind
fails with `Can't assign requested address`.

So right after a restart, a `100.x` URL can be briefly unreachable while the
local one works. `GET /~/api/v1/health` lists addresses still awaited as
`pending_binds`, empty once all are bound. `loopback` and `all` are unaffected:
their single address is required, and failing to bind it is a hard error
(the port is held, not an interface missing).

## Operational notes

- **The host must be awake.** A sleeping Mac serves nothing. Lucidos holds it
  awake while work runs (a turn, a coding-agent session, a background task, a
  backup; ADR 0366), and an idle Mac still sleeps. The telltale: everything
  works at the desk and dies when the user walks away. For an idle host that
  must stay reachable, use System Settings → Battery/Energy → prevent automatic
  sleeping when the display is off (mains power only). Closing the lid on
  battery sleeps regardless.
- **Resolve Tailscale CLI/daemon version skew before debugging `serve`.** A
  mismatch (for example CLI 1.96.4 against daemon 1.98.9) prints a warning on
  every command. Fix it first: `serve` semantics shift between versions, and the
  warning buries the real error. `tailscale version` prints both halves.
  `brew upgrade tailscale`, or updating the app and reopening the shell,
  resolves it.
- **Suggest Add to Home Screen once HTTPS works.** On iOS: Safari → Share → Add
  to Home Screen. The user gets a full-screen icon with no browser chrome, plus
  web push. Offer it unprompted: it is the payoff for TLS, and most users do not
  know to ask. Add the workspace URL (`https://<host>/<slug>/`) rather than the
  root if they live in one workspace. The home-screen app pairs separately from
  Safari: see § A phone installs before it pairs.
- **The URL to hand over.** The gateway root 307-redirects to the sole workspace
  or the picker (`/~/`). A direct workspace link is
  `https://mymac.tailnet-name.ts.net/<slug>/`. Settings → Access prints exactly
  that under Connect URLs, with a Copy button, so point the user there.

## Quick triage

| Symptom | First thing to check |
|---|---|
| "Not Secure" label | Route A is in use. Move to B, or C. |
| Phone loads nothing at all | Bind (loopback only), host asleep, or the device is not on the tailnet. |
| Certificate error naming a different host | SAN list is missing the MagicDNS name. `openssl x509 ... -text`. |
| "Not trusted" on one device only | iOS trust step 4 (Certificate Trust Settings) was skipped. |
| `500 ... your Tailscale account does not support getting TLS certs` | The account-level HTTPS toggle is off. Nothing local will fix it. |
| `tailscale serve` prints a `login.tailscale.com/f/serve` link and never returns | Serve is not enabled for the tailnet. Open that exact link and approve; the command finishes by itself. Not a hang. |
| Expose reports a CLI syntax change on a current CLI | Read past it. That line comes from the pre-1.52 fallback attempt, which now runs only on a rejected flag; if you see it on 1.52+, the build predates that gate. |
| Works on 5252, fails on 5251 (or vice versa) | Two gateways, two TLS setups. Probe the failing port on its own. |
| No push notifications | Not a secure origin (check that first), or the OS-level permission was never granted. |
| Apps load blank behind a reverse proxy | A path prefix was added. Serve Lucidos at the origin root, on its own port. |
| `tailscale serve status` says "No serve config" but the URL works | Route C is in play. Not a problem. Probe the port. |
| Anything needing a live socket fails, while pages load | A hop is dropping the WebSocket upgrade. Probe `/<slug>/api/v1/ws-echo`, which upgrades and echoes what you send it. A `101` means every hop carried it. |
