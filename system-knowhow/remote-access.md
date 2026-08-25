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
   decides the "Not Secure" label and whether service workers, web push, and
   PWA install are available at all.

Conflating them is the usual source of confusion. A tailnet address with a
perfect tunnel still shows "Not Secure" if the origin is `http://`, and a
perfect certificate is useless if the gateway is bound to loopback and nothing
proxies to it.

## Diagnose first, never assume

**A machine commonly runs more than one Lucidos gateway, and they can have
completely different TLS setups.** The standard collision is the packaged
`Lucidos.app` on **5252** (which serves plain HTTP by default) alongside a dev
gateway from a source checkout on **5251** (which serves **https** whenever the
checkout has `.certs/cert.pem` + `.certs/key.pem`). Advice derived from one of
them is wrong for the other. Probe before you say anything.

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

Expect several rows: one `lucidos-gateway` per install, plus one
`lucidos-engine` per running workspace on its own port. **Every engine should
read `127.0.0.1`**, packaged and dev alike. The gateway is the only
network-facing surface, because it is the only one that authenticates its
callers. An engine row on `*:` or a `100.x` address is a workspace reachable
with no credential, and means something set `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0`.
Hand a remote device the **gateway** port: it routes to every workspace by slug.

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

`-k` on the https probe is deliberate: it answers "is TLS spoken here" without
mixing in "is the certificate trusted". Those are separate questions with
separate fixes. Check trust separately, by name, from the device's point of
view:

```bash
curl -sv https://mymac.tailnet-name.ts.net/ 2>&1 | grep -i 'subject\|issuer\|SSL certificate'
```

### Step 3: repeat from the remote device's viewpoint

Probe the **MagicDNS name**, not `127.0.0.1`. A cert can be valid for
`localhost` and useless for `mymac.tailnet-name.ts.net`, and loopback probes
will never reveal that.

## Devices pair before Lucidos answers them

**The gateway authenticates every caller that reaches it over the network.** An
unpaired device is answered with a pairing screen, at the address it asked for.
So reaching the port is no longer the same as being the user.

**The desktop app pairs itself.** Open it and you are in. Its Rust side reads
the machine-local token, which a browser cannot, so it mints a code and spends
it without asking. That is the first device on a fresh install, and it takes no
terminal.

Any paired device can then let the next one in. **Settings → Access → Add a
device** mints a code and draws a QR the phone can scan.

A terminal does the same, and is the fallback when nothing is paired at all:

```bash
lucidos pair            # prints a code; type it into the device
lucidos pair --qr       # draws it as a QR the phone can scan
lucidos pair --port 5251  # when two gateways are running, say which one
```

It finds the gateway by probing. With two running it refuses rather than
guessing, and names the ports it found: a code only works on the gateway that
minted it.

`lucidos` is on no `PATH`. A desktop install keeps it inside the app bundle, at
`Lucidos.app/Contents/Resources/lucidos`. A headless install keeps it under the
install prefix, in `runtime/current/`.

The code works once and expires in five minutes. A device stays paired until
you revoke it, and until nothing else: there is no idle timeout and no absolute
one. An expiry would cut off only the devices you forgot, since a credential in
use never goes stale. Revoking answers the device you know you lost.

**Devices says when it last saw each one**, to the nearest day. That is what
tells a phone in daily use from a laptop you sold, so read it before you
revoke. The browser's cookie carries its own window, refreshed on each day a
device is seen, so an active device never reaches it. That window is a
convenience: the gateway never reads it, and age is not an input to the auth
decision.

Five things about it are worth knowing, because each surprises people:

- **A device pairs to a GATEWAY, not to a machine.** Almost every install runs
  one gateway, so the distinction never shows. It shows on a machine running
  two, which is supported: the packaged app on 5252 beside a dev checkout on
  5251. Each keeps its own device list and its own codes. So a code minted on
  one is refused by the other, and Settings → Devices lists the gateway serving
  that page. Pair the phone against the address it actually reaches. ADR 0132
  says why the local token stays shared while the device list does not.
- **A browser pairs even on that machine.** Proving you are local means reading
  a file only your user can read, and a browser cannot read files. So Safari on
  the host pairs exactly like a phone does. Two things never pair, because both
  attach that proof themselves: the CLI, and the desktop app's own Rust side.
- **Being on the tailnet is not what authorizes you.** Auth reads no Tailscale
  header and works the same over `tailscale serve`, mkcert, or a plain LAN
  address. The tailnet is transport.
- **A workspace's own engine port is not a way in.** Engines bind loopback, so
  only this machine reaches one. Every other device goes through the gateway at
  `/<slug>/` and pairs. A bookmark straight at an engine port stops resolving
  from elsewhere, which is the point: it was a way around pairing.
- **Apps in an *app UI* iframe still act with your authority**, exactly as they
  did before. They are served same-origin and share the browser's session. An
  app cannot copy the credential off the machine, but it can still call the API
  as you.

### Still do not put Lucidos on the open internet

Authentication raises the floor; it does not make a public origin a good idea.

- Use `tailscale serve` (tailnet-private), **never `tailscale funnel`** on the
  gateway's own port.
- No router port-forward, no public reverse proxy, no ngrok-style tunnel.
- Keep the tailnet as the outer boundary, with pairing as the inner one.

### The one exception: a webhook's own socket

A *webhook* is the single surface meant to be reached by someone who will never
join your tailnet. GitHub cannot pair. So webhook deliveries answer on their
own port, the *hook socket*, and that port is the only thing you point
`tailscale funnel` at (ADR 0097).

**The isolation is structural, not a rule to follow.** Funnel maps a *port*,
never a path, so "expose only the webhooks" is not something it can express.
The hook socket has exactly one route, `POST /<slug>/<webhook-id>`, and answers
404 to everything else, a wrong method included. A public caller therefore
reaches no control plane and no workspace, whatever it asks for. Pointing funnel
at the gateway's own port would put both one auth bug away from the internet.

The hook port is the gateway's plus ten: **5261** in dev, **5262** packaged.
`LUCIDOS_HOOK_PORT` overrides it, and `0` switches the socket off entirely.

```bash
tailscale funnel --bg 5262   # packaged: publish ONLY the hook socket
tailscale funnel status      # what is public right now
tailscale funnel off         # stop publishing
```

`tailscale funnel --help` is the authority for the installed version, exactly as
`serve --help` is below. Deliveries then arrive at
`https://<machine>.<tailnet>.ts.net/<slug>/<webhook-id>`.

**A public port is not an open door.** Every delivery still authenticates, by
bearer token or by the sender's own signature. Each webhook emits one pinned
event, fixed when you created it. Create one with `lucidos webhooks create`;
`system-knowhow/lucidos-cli.md` covers the tokens and the GitHub, Slack and
Stripe signature shapes.

**A sender out there will resend.** GitHub retries a slow response, Stripe
retries for days, and by default each arrival emits again. `--dedupe` names the
header carrying the sender's delivery id and collapses the repeat. The CLI page
covers it, along with the `--headers` allow-list that puts a chosen request
header in the payload.

## Why HTTPS matters, and when it does not

Browsers gate a set of features on a *secure context* (https, or anything on
`localhost`):

- **Service workers**, and with them **web push notifications** and reliable
  **PWA install / standalone launch**.
- Clipboard API, Notification API, geolocation, media capture.

Plain `http://` to a tailnet name is not a secure context, so all of the above
are unavailable and Safari/Chrome show a "Not Secure" label. If the user only
wants to read and chat from a laptop and does not need push, that is a
legitimate choice. If they want the phone to behave like an app, they need one
of the two TLS routes.

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
tailnet is private. So the user's instinct that `http://` over a tailnet feels
weird is worth answering honestly: **confidentiality here comes from the tunnel,
not from TLS**, and it is real. But the browser has no idea a tunnel exists. It
sees an insecure origin and applies the full penalty: the "Not Secure" chip,
no service worker, no push, no clean PWA install. The UX cost is real even
though the security worry is not.

Requires the gateway to be bound beyond loopback (see § Network bind, below).

### Route B: `tailscale serve` (recommended)

Tailscale terminates TLS itself, with a **real Let's Encrypt certificate for the
MagicDNS name**, renewed automatically. Nothing to install or trust on any
device: phones, tablets, and other laptops just work, on or off the LAN. The
gateway can stay bound to loopback, because `serve` proxies from this machine to
`127.0.0.1`.

**Prerequisite the agent cannot satisfy: Serve, and tailnet HTTPS, are
account-level and enabled in a browser.** Only a tailnet admin can turn them on.
This shows up in two different ways depending on which layer you hit first, and
the two look nothing alike, so recognise both.

**1. `tailscale serve` prints a link and then blocks.** On a tailnet that has
never had Serve enabled, the CLI does not fail. It says this and then waits,
polling the control plane until someone visits the link (measured 2026-08-02 on
CLI 1.96.4):

```
Serve is not enabled on your tailnet.
To enable, visit:

         https://login.tailscale.com/f/serve?node=<node id>
```

The node id is per-machine and cannot be reconstructed, so **that exact line is
the whole answer**: open it, approve, and the still-running command finishes by
itself. `tailscale status --json` shows the same precondition from the outside
as an empty `CertDomains`.

**`--yes` does not help.** It suppresses interactive *prompts*, and this is not
a prompt. Tested: with `--yes` the command blocks identically. Nor does closing
stdin. The only thing that unblocks it is the approval.

**2. Cert provisioning fails outright**, if Serve is enabled but tailnet HTTPS
certificates are not: <https://login.tailscale.com/admin/dns> → **HTTPS
Certificates** → **Enable HTTPS**.

```
500 Internal Server Error: your Tailscale account does not support getting TLS certs
```

Recognise that string immediately. It is **not** a Lucidos fault, not a network
fault, and not transient. Retrying, reinstalling Tailscale, restarting the
gateway, and regenerating anything will all fail the same way. Stop and ask the
user to enable HTTPS in the admin console (or fall back to Route C if they
cannot, e.g. they are not an admin of that tailnet).

Setup:

```bash
tailscale up                                             # once, interactive sign-in
tailscale serve --bg --https=443 http://127.0.0.1:5252   # front the gateway
tailscale serve status                                   # verify the mapping
tailscale cert mymac.tailnet-name.ts.net                 # provision/inspect the cert
```

The packaged desktop app does this for the user: **Settings → Access**
runs `tailscale serve --bg --https=443 http://127.0.0.1:<port>` behind its
**Expose** button, waits out the tailnet approval above if one is needed, and
then shows the resulting `https://mymac.tailnet-name.ts.net` URL. Prefer
pointing the user there over hand-running commands when they are on the packaged
app. See § Settings → Access below for what the page can and cannot do,
and § The Expose run for the steps it goes through.

**`serve` syntax changed in CLI 1.52, and the old form has since been removed.**
Two forms exist in the wild and a given CLI takes one of them:

| Form | CLI | On the wrong CLI |
|---|---|---|
| `serve --bg --https=443 <target>` | 1.52 and later | unrecognised flag |
| `serve https / <target>` | before 1.52 | `Error: the CLI for serve and funnel has changed`, exits non-zero, configures nothing |

`--bg` arrived with the same 1.52 rework, so it belongs only on the first form:
before that, `serve` was persistent by default. The Expose button tries the
current form and falls back to the old one (`serve_arg_forms` in
`crates/lucidos-app/src/mobile.rs`). If both fail it reports **both** errors,
current first with the retry labelled, because whichever form the CLI does not
understand contributes only noise: on a current CLI that is the legacy attempt's
"the CLI for serve and funnel has changed", and on a pre-1.52 one it is the
current attempt's unknown flag. Keeping both is what stops the real reason from
being the one that gets dropped.

**The fallback runs only when the CLI rejected a FLAG**, never on any other
failure. That gate is load-bearing rather than tidy. It used to retry on
anything, so on a modern CLI a run that timed out waiting for the tailnet
approval collected "Error: the CLI for serve and funnel has changed" from the
doomed legacy attempt and led with it, sending the reader after a syntax problem
they did not have while the approval link went unmentioned. A deadline is not
the CLI declining our argv.

**`--bg` belongs only on the current form, and an attempt has TWO deadlines.**
The 1.52 rework did not just add flags, it inverted the default: before it,
`serve` wrote persistent config and had no foreground concept (so no `--bg`);
after it, `serve` holds a foreground session until Ctrl-C unless you pass
`--bg`. A CLI in the window between that rework and the removal of the old
syntax therefore accepts `serve https / <target>` **and** runs it in the
foreground, so that invocation never returns on its own. Hand-running the old
form on a recent CLI, expect it to sit there rather than come back.

Hence 20 seconds while the command should be *configuring*, which is a stall
guard, and ten minutes once it has printed the tailnet-approval link, which is a
patience budget: at that point it is not stalled, it is waiting for a human, and
it completes on its own. The child's output is streamed while it runs rather
than read after it exits, so an attempt that is killed still reports everything
it had said. Reading the pipes only at exit is what once discarded the approval
link along with the child.

Hand-running, `tailscale serve --help` is the authority for the installed
version and `tailscale serve status` is the proof of what actually got
configured. When a newer CLI rejects the old form it **prints the exact command
it wants**, so read the error instead of guessing at syntax.

**Port 443 fronts exactly one target.** With two gateways, one takes 443 and the
other takes an alternate port:

```bash
tailscale serve --bg --https=443  http://127.0.0.1:5252   # packaged install
tailscale serve --bg --https=8443 http://127.0.0.1:5251   # dev checkout
```

The phone then opens `https://mymac.tailnet-name.ts.net` and
`https://mymac.tailnet-name.ts.net:8443`. Both get the same certificate, since
it covers the DNS name, not the port.

**Do not path-prefix proxy** (`/dev/` → 5251) to squeeze two gateways onto 443.
It breaks in a way that looks like a Lucidos bug: the gateway already owns the
first path segment as the workspace slug (`/<slug>/`, with `/~/` reserved for
the picker), and apps served in the iframe assume they live at the origin root.
An extra prefix corrupts `<base href>`, app asset URLs, and the SDK's
`/api/v1` calls. Use a second port instead.

### Route C: mkcert (Lucidos terminates TLS itself)

Use when tailnet HTTPS is unavailable, when access is LAN-only without
Tailscale, or on a dev checkout where the certs already exist.

**The MagicDNS name must be in the certificate's SAN list**, alongside
`localhost` and the tailnet IP. Generate with every name a device might type:

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

If Tailscale is not up, or `en0` has no address (wired-only machine, Wi-Fi off),
that substitution comes back **empty** and mkcert fails on the empty argument.
Run the substitutions on their own first, then pass the values you actually got.

Verify before blaming anything else:

```bash
openssl x509 -in .certs/cert.pem -noout -text | grep -A1 'Subject Alternative Name'
```

A missing MagicDNS name surfaces on the phone as a name-mismatch error, which
reads like a trust failure and is not one. Check the SANs before walking anyone
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

**The critical caveat: a mkcert certificate is signed by a local development
CA.** Only machines that trust that CA will connect. Every additional device
needs the root installed *and* trusted:

1. `mkcert -CAROOT` prints the directory holding `rootCA.pem`.
2. Transfer `rootCA.pem` to the device (AirDrop is easiest for iOS).
3. Open it on iOS, then **Settings → General → VPN & Device Management** and
   install the downloaded profile.
4. **Separately** go to **Settings → General → About → Certificate Trust
   Settings** and toggle the mkcert root on.

**Step 4 is the one everybody misses.** Installing the profile alone leaves the
CA present but untrusted, and Safari keeps rejecting the certificate. When a
user says "I installed the certificate but Safari still says it is not
trusted", ask about Certificate Trust Settings first, before regenerating
anything.

Other mkcert gotchas:

- Restart Chrome after the first `mkcert -install`; it caches the CA store.
- Regenerate (and restart the gateway) whenever the LAN IP changes or the
  certificate expires, and repeat the trust steps on every new device.

### Fourth option, when the other device has a terminal

`ssh -L 5252:localhost:5252 <host>` then `http://localhost:5252` gives the full
app including push, with no certificate at all, because **localhost is a secure
origin**. Useless on a phone, ideal for a second laptop.

## Settings → Access

The page that drives all of this. Point the user here before hand-running
commands. It was called **Mobile Access** until 2026-08-05, when the **Network
access** bind setting (previously a Settings → System subpanel this page had to
deep-link into) moved onto the bottom of it: reaching this engine from another
device is one question, and its two halves now sit together.

**It answers two independent questions, and never muddles them.** They are
numbered on the page in the order the setup happens, because the first has to be
true before the second buys anything:

| Section | Question | Where the answer comes from |
|---|---|---|
| 1. The machine running Lucidos | Is the engine's machine on a tailnet? | `detected_tailscale_ip` from `GET /api/v1/network-config` in any browser; the fuller `get_connect_info` probe on the packaged desktop app |
| 2. This device | Has the device reading the page joined that tailnet? | The address this device was served on |

**Add a device is not one of the two questions**, it is the action they lead
to: pairing a new device at one of those addresses. See its own section below.

Every section renders **everywhere**, phone browsers and the installed PWA
included, Connect URLs among them. What varies by platform is how much each can
say, never whether it appears. Only the **actions** are gated: Sign in to
Tailscale and Expose are native commands with no HTTP equivalent, so they exist
on the packaged desktop app alone. **Get Tailscale** is a link rather than a
bridge call, so it is offered wherever it can be acted on. It opens the App
Store on iOS, the Play Store on Android, and `tailscale.com/download` otherwise.

Do not restore the old shape, where the page chose **one** of the two by
platform. A browser then saw section 2 alone, and a machine whose gateway is
bound to its own tailnet address serves every remote device at a bare `100.x`
host, so all of them were told to install Tailscale over a connection that only
existed because Tailscale was working, while section 1 (which would have shown
them the address) was the half being suppressed.

**Section 2 reads the device it is running on**, so it never asks a device to
redo a step it has already done. There is no Tauri bridge on a phone and no way
to inspect its interfaces from a web page, so the evidence is the host it was
served on, checked three ways:

| How this device got here | The page shows |
|---|---|
| Loopback (`localhost`, `127.0.0.0/8`, `::1`, `*.localhost`) | "You are reading this on the machine that runs Lucidos". No install offer and no app-install step: this device IS the machine, so section 1 is its whole answer |
| A `*.ts.net` name | "Tailscale is connected on this device" |
| The exact `100.x` address section 1 reports for the machine | the same, for the same reason |
| Any other address | **Get Tailscale**, then all three steps (install Tailscale, copy the machine's Tailscale address, install the app) |

**Being on the tailnet is not the last question.** Which step remains also
depends on whether the origin is secure (`window.isSecureContext`), because the
installable app and push are gated on that and Route A's `http://` tailnet
address is not one:

| On the tailnet, and… | The remaining step |
|---|---|
| a secure origin (`https`, or loopback), in a browser tab | install the app here |
| plain `http://`, in a browser tab | **none, on this device.** The page says so and points at `tailscale serve` on the machine, because the browser will offer no install control here however the page words it |
| already the installed app | nothing |

Each proof is sound on its own. A MagicDNS name resolves only on a device signed
in to the tailnet that owns it. A request that arrived at the machine's tailnet
address arrived over that tailnet, and the address is trustworthy because the
engine read it off a Tailscale **interface** (`lucidos_tailscale::tailnet_ipv4`
requires the interface *and* the range), not out of the range. And a loopback
request never left the machine.

A bare `100.64/10` host that does **not** match the reported address is still
deliberately not proof: that range is real CGNAT space an ISP can hand to a
physical interface, and the interface check cannot be run against a remote
device from here. An unproven host keeps the install offer, which is the
harmless way to be wrong.

**The page never calls a device a phone unless it is one.** It is read from a
desktop browser as often as from a handset, and a desktop browser has no home
screen, so the "add to home screen" wording is behind an iOS/Android check and
the neutral phrasing is "install Lucidos".

**Connect URLs** lists the addresses that reach **this workspace**:

- **This Mac**: `http://localhost:<port>/<slug>/`. A secure origin, so a full
  PWA works here, which is Route D. Packaged desktop app only, since the
  localhost port comes from the Tauri bridge.
- **Local network**: shown only when the gateway is bound beyond loopback. Plain
  HTTP, so no PWA install and no push. Bound to loopback (the packaged default)
  the row points at the **Network access** section further down this same page
  instead of printing a dead URL. Packaged desktop app only, since detecting a
  LAN address needs the bridge.
- **Tailscale**: the tailnet address over plain HTTP until `serve` is verified,
  and `https://<name>.ts.net/<slug>/` once it is. Rendered **everywhere**,
  including a phone browser: see § The tailnet-status endpoint.

**Every row carries the `/<slug>/` prefix**, because that is what addresses a
workspace (ADR 0014). A bare origin reaches the gateway root, which
307-redirects to the sole workspace or to the picker. On an install with more
than one workspace that is the wrong address to hand out.

Both plain-HTTP rows obey the network bind, and for the same reason: being on a
tailnet does not mean the gateway is **listening** on the tailnet address. Under
the packaged loopback default neither prints, because both URLs would be dead.
A bind pinned to the tailnet address shows the Tailscale row and reports the LAN
as off, which is accurate: that bind serves one address, and it is not a LAN one.
`serve` is unaffected by all of this, since it proxies from this machine to
`127.0.0.1` and needs no wider bind. So the **HTTPS row is never bind-gated**,
and it is the one row that survives the packaged default.

The bind weighed is the one belonging to whichever process served the page.
Behind the gateway that is `gateway_bind`. On a direct engine port the origin is
the engine, which follows the gateway only while `[engine] inherit` is on.

### Add a device

Under Connect URLs, and the thing you DO with one of those addresses. It mints
a *pairing code* and draws it as a QR, so a phone scans rather than typing
eight digits. Whoever is reading the page is already paired, which is what
makes the offer safe: a paired device holds full authority and may enrol
another.

**A live code is shown as the three ways to use it**, one card each: scan the
QR, type the digits, open the address. Scanning is the point, so the QR is the
big card and the two fallbacks stack beside it. The digits are always there, so
a failed scan is never a dead end, and they are set large enough to read across
a desk.

They are alternatives, not steps. Every card after the first says "Or", since a
row of imperatives reads as a checklist of three things to do. The two fallbacks
carry a Copy button, hidden where the browser exposes no clipboard (a plain-HTTP
LAN address is not a secure context).

The address card is text, never a link. It is meant for the other device, and
following it here would spend a single-use code on a device already paired.

**The QR encodes `<reachable-origin>/~/?pair=<code>`.** Scanning it opens the
picker, whose pairing screen reads the parameter, fills the code in and strips
it from the address bar. A code works once and expires, so a URL still carrying
one is a URL that will stop working: a reload, a bookmark or a shared link must
not keep it.

On a phone the scan lands somewhere else, and § A phone installs before it pairs
says why.

**The address is the hard part.** The machine minting a code is usually reading
this page over loopback, and a QR aimed at `127.0.0.1` helps nobody. So the
section takes the same derivation the Tailscale row above uses: the verified
`serve` origin, else the MagicDNS name, else the tailnet address, and only
while something is listening on it. Then the LAN address, on the packaged
desktop app where one can be detected. With none of those it mints the code and
says there is no QR, rather than encoding an address that cannot work.

**The expiry is a live countdown**, not a sentence claiming five minutes an
hour after the fact. It sits above the cards with the New code button, since it
is true of the code rather than of one way to spend it. Single use is not
repeated there: it is a fact nobody has to act on, and the line is re-read every
second.

Once a code expires the cards go, leaving the countdown's verdict and the
button. Every card is an instruction to use a code that no longer works, and
the button is the way out of that state.

**Nothing here mints a code by itself.** A phone that installs Lucidos and comes
back to scan again may find the code expired, and the reader presses the button
for another. The section used to replace an expired code on its own, which read
as the page undoing the press the reader had just made. See ADR 0098.

The section is gated on `/~/…` reaching the gateway, which is true exactly
while the page is served under `/<slug>/`. A page served straight off an engine
port resolves that path against the engine and gets a 404, so it says to run
`lucidos pair` instead. It never hides: the heading is a Search Everywhere
destination, and an absent section drops that hit at the top of the page.

### A phone installs before it pairs

**On iOS the home-screen app is a different device from Safari.** It gets its
own storage container, so the credential cookie taken in a Safari tab never
reaches it. iOS also cannot route a scanned link into an installed web app, so
the Camera app always hands it to Safari. Pairing the tab therefore enrols the
wrong thing and leaves the app locked out. Android does not have this problem,
because an installed PWA captures links inside its own scope.

So the pairing screen shows a phone browser the **install steps** rather than
the code form. Add Lucidos to the home screen and open it: the code rode into
the manifest's `start_url`, and the app spends it on sight with nothing typed.
Somebody who wants the browser paired anyway taps **Pair this browser instead**.

**That code is fixed at install time, and it still lasts five minutes.** An
install slower than that opens on the pairing screen with the code refused, and
the two routes below are how it recovers. A fresh code on the host does not
reach it: the app's launch URL was written when it was installed.

**An app already on the home screen cannot be reached that way**, since its
launch URL is fixed. Two ways across for that case, both on the pairing screen
inside the app:

- **Paste code.** The browser screen offers Copy, and the pasteboard is shared.
- **Scan QR.** The app opens its own camera and reads the QR off the host's
  screen. It appears on a phone over HTTPS, where a camera can be opened at all.
  An expired code leaves no QR to read, so the host makes a fresh one first.

Typing the eight digits still works everywhere, and is what a desktop browser
does.

### The list of devices lives in Settings → Devices

Access adds a device. **Settings → Devices** is where every device is listed,
and where **Revoke** is. One row per device, carrying both of the things you
can do to one:

| Action | Reach | What it does |
|---|---|---|
| **Revoke** | the whole machine | Stops this device reaching Lucidos, on every workspace. |
| **Remove** | this workspace | Forgets its push subscription and its preferences here, and leaves it paired. |

They are two buttons because they answer different questions, and they used to
be two lists for the same reason. That was the problem: the same phone appeared
under **Paired devices** and again under **Devices**, with different names, and
neither row knew about the other. Both now key on the id the gateway minted
when the device paired, so there is one device and one row.

A row can be missing either half, and neither is an error. A device paired from
another workspace holds nothing here yet, and its row says **Not set up in this
workspace**. It keeps a push toggle, switched off and disabled, because push
hangs off the engine row it does not have yet. A browser on a direct engine port
never went through the gateway, so it has nothing to revoke. With no gateway at
all the pairing column is dropped rather than guessed at, and Search withholds
the **Revoke** hit on such a page.

The row states the present and never a history it cannot see. Nothing there
claims a device has never opened this workspace, because a missing engine row
does not prove that. **Remove** deletes the row of a device sitting right in
front of you. And a device that paired before its two ids were unified keeps
them apart until it next loads the page.

### What a device is called

Each half carries its own name, and the row prefers the one you can edit.

The pairing screen offers a name it reads off the browser, such as `Chrome on
Mac` or `Safari on iPhone`. The person at the device may overwrite it, and that
typed name wins: whoever is holding the device is being more specific than
whoever minted the code. So `lucidos pair --label` is a fallback. It applies
when the field is left empty, and the CLI says as much when it prints the code.
An unrecognised browser suggests nothing and leaves the field blank, which is
what keeps the fallback reachable. With neither, the device is listed as
"Paired device".

That pairing name is fixed: revoke and pair again to change it. The name on the
**Devices** row is not. Click it and type, and that is what the row shows from
then on. A device with no engine row yet shows its pairing name instead.

A device with neither is listed as `device-` plus the first eight characters of
its id, which is what an actor chip calls it too. The whole id is never the
heading: it is unreadable, and at that length it wraps onto a second line.

### The tailnet-status endpoint

`GET /api/v1/tailnet-status` is what puts the Tailscale row in a browser. It
returns two fields, each a string or null:

| Field | Meaning |
|---|---|
| `magic_dns_name` | `<machine>.<tailnet>.ts.net`, no scheme. Null off a tailnet, and null with MagicDNS turned off |
| `workspace_serve_url` | The `https://<name>/<slug>/` URL, published only once verified |

The name is the point. It is a reverse lookup only the machine can run, so a
browser has no other way to learn it. It is also the address a user copies to
another device. The plain-HTTP row prefers it over the bare `100.x` address for
the same reason: it resolves to that address anywhere on the tailnet, and a
person can retype it.

**`workspace_serve_url` is verified end to end, never inferred from a
listener.** A TCP probe of 443 proves that something serves HTTPS and says
nothing about which gateway. Two gateways on one machine is a documented setup:
443 fronts 5252, 8443 fronts 5251. So a live 443 can belong to a gateway that
has never heard of this slug. The engine therefore fetches the candidate URL's
own `api/v1/health` and compares the `workspace_path` it reports with its own.

A same-named workspace on the other gateway lives at a different path, which is
what makes the comparison a proof. The probe validates TLS normally, because
Tailscale issues a real certificate for a `.ts.net` name. It costs nothing when
the machine is off a tailnet, and both halves are bounded.

It is deliberately a separate route from `network-config`. The bind editor
fetches that one too, and must not pay for a reverse lookup and a network round
trip.

The packaged app's own `serve_url` (from `get_connect_info`) is a different fact
and stays where it is. It answers "is this MACHINE serving", which is what the
Expose row reports. The endpoint above answers "what is the URL for THIS
workspace".

**Tailnet state is read without the Tailscale CLI.** The page takes the tailnet
address from the machine's own interface list and the MagicDNS name from a
reverse lookup, so it reports correctly no matter how Tailscale was installed
(App Store, standalone app, or Homebrew), and on a packaged process that has no
`PATH`. Consequences worth knowing when debugging:

- A machine with **MagicDNS disabled** has a tailnet address and no name. That is
  still "on the tailnet"; it just has no HTTPS name to serve.
- A CLI is needed for **`tailscale serve` only** (and for the Sign in button).
  `serve` has no GUI, config-file or admin-console equivalent, so a Mac without a
  CLI can be described accurately but cannot be exposed. The page says so, and
  names the two ways to get one: Install CLI in the Tailscale app, or
  `brew install tailscale`.

**The four states of section 1 on the packaged desktop app**, from those two
independent facts. In a browser the same section reports the tailnet half of
this and offers no action, because every action below is a native command:

| Tailnet state | CLI | The page shows |
|---|---|---|
| Tailscale absent | any | **Get Tailscale** |
| Installed, not on a tailnet | yes | **Sign in**, with an optional auth key |
| Installed, not on a tailnet | no | Sign in from the Tailscale menu-bar app |
| On a tailnet, not serving | yes | **Expose** |
| On a tailnet, not serving | no | How to get the CLI; the plain-HTTP URL works meanwhile |
| On a tailnet, serving | any | The `https://...ts.net` URL, plus **Re-apply** with a CLI |

**A failed Sign in or Expose shows the underlying error verbatim.** The toast
carries no Lucidos framing of its own, because every message the page can raise
already names what failed: the missing CLI, the missing tailnet address or
MagicDNS name, `tailscale <cmd> failed: <stderr>`, or a post-condition that
reported success and changed nothing. So read the toast as the CLI's own words.
That matters most for a syntax rejection, where the CLI prints the exact command
it wants and that line **is** the fix.

One thing is filtered out of those messages: `Warning: client version "..." !=
tailscaled server version "..."`. Tailscale prints it on stderr for *every*
command whenever the CLI and the running daemon differ in version, which is the
normal state of a Homebrew CLI beside the Mac app's daemon, and with the child's
output streamed it would otherwise be the first line of every error. It is still
worth resolving before debugging `serve` in earnest (see the troubleshooting
table), it just is not the error.

### The Expose run

Pressing **Expose** starts a supervised run, not a single blocking call. It
reports on the **brand badge** in the header (the shared background-activity
surface, alongside a dev engine rebuild and the embedding-model download): the
badge spins for the whole run from any screen, and tapping it opens the status
toast with the current step. Every step is indeterminate, so the toast spins
rather than showing a bar.

| Step | What is happening |
|---|---|
| Setting up Tailscale access | Locating a CLI, then reading the tailnet address and MagicDNS name |
| Configuring tailscale serve | The `serve` command is running |
| Waiting for you to enable Serve on your tailnet | The CLI printed an approval link (see Route B). The toast offers it as **Enable in Tailscale**; approve it in the browser and the run continues on its own |
| Waiting for HTTPS to come up | The mapping is written; polling 443 for up to 30s, because a first-run certificate takes a moment |

The run ends as the `https://...ts.net` address, an error shown verbatim, or
nothing at all if it was cancelled. **Cancel** is offered throughout, and a
cancelled run leaves no serve config behind (killing the child before it commits
writes nothing). Only one run exists at a time: pressing Expose again while one
is live is refused rather than racing it, and the button reads "Setting up…" and
stays disabled even if you navigate away and come back, because the run lives in
the app and not on the page.

Two things this means when debugging a report of "Expose did nothing". First,
the run may simply be waiting for the user on Tailscale's site, and the badge is
where that is said. Second, nothing here can occupy the UI thread: every Mobile
Access command runs on a worker thread, so a frozen window is a different bug.

**Never run `/Applications/Tailscale.app/Contents/MacOS/Tailscale`.** It is the
GUI executable, not a CLI. Outside a GUI session it prints "The Tailscale GUI
failed to start ... (Tailscale.CLIError error 3)" and **exits 0**, so anything
checking the exit code reads it as success with unparseable output. That is a
shipped bug, not a hypothetical: it is why Mobile Access once showed a Sign in
button that reported success and changed nothing on a Mac already on its
tailnet. Use `/usr/local/bin/tailscale` or `/opt/homebrew/bin/tailscale`.

## The detection trap: Tailscale state does not tell you whether HTTPS works

If the gateway terminates TLS itself (Route C), **Tailscale is not in the
request path at all**. In that setup:

- `tailscale serve status` prints `No serve config`.
- `tailscale cert` may be entirely unprovisioned.
- And `https://mymac.tailnet-name.ts.net:5252/` works perfectly.

An agent that checks only Tailscale-side state concludes "HTTPS is not set up",
then walks the user through configuring something they already solved a
different way, or worse runs `tailscale serve` and shadows a working setup with
a second, half-configured one.

The inverse trap exists too: a `serve` mapping that *exists* does not prove it
works, because the certificate may never have provisioned (the account toggle
above).

The Access page follows this rule rather than guessing, in two places, and they
are not the same probe.

The **Expose / Serving row** reports this MACHINE's serve state, on the packaged
desktop app, from `get_connect_info`. It claims it only after probing **port 443
on the tailnet address** and finding something answering. It previously claimed
it the moment a MagicDNS name resolved, which is well before anything is
listening. That probe proves a **listener**, not a working certificate, so the
row can appear while a first-run cert is still provisioning.

The **Connect URLs Tailscale row** asks the harder question and gets a harder
answer: see § The tailnet-status endpoint. It fetches the candidate URL's own
`api/v1/health` with TLS validated, so a listener is not enough and neither is
a certificate. Until that succeeds the row shows the plain-HTTP tailnet address,
which is honest about what works today.

Neither asks `tailscale serve status`. That answers "does *a* serve mapping
exist", which is a different question: with only the 8443 mapping from the
two-gateway setup above, the config is non-empty while `https://<name>` on 443
is still dead. Each surface tests exactly the endpoint it is about to name.

**Rule: the port is the source of truth.** Probe it directly before making any
claim about whether HTTPS works:

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

Binding to the tailnet IP specifically (`100.x.y.z`) is the middle ground: the
tailnet reaches it, the coffee-shop LAN does not.

**A configured IP that is not up yet does not hold the start back.** An `<IP>`
bind is always accompanied by loopback, and only the loopback half is required:
the gateway serves on it immediately and retries the configured address in the
background until the interface appears, then starts listening on it too, with no
restart. This matters at boot, where launchd starts the service before
`tailscaled` has assigned the machine's `100.x` address, so binding it fails with
`Can't assign requested address`. Until 2026-08-07 that failure was fatal to the
whole gateway, and the desktop window sat on its startup splash for two minutes
waiting for a process that had already exited.

So a `100.x` URL can be briefly unreachable just after a restart while the local
one already works. `GET /~/api/v1/health` reports any address still being waited
on as `pending_binds`; an empty array means everything configured is bound.
`loopback` and `all` are unaffected: their single address is required, and
failing to bind it is still a hard error (that failure means the port is held,
not that an interface is missing).

## Operational notes

- **The host must be awake.** A sleeping Mac serves nothing, and the symptom is
  distinctive: everything works while the user is at their desk and dies the
  moment they walk away. Fix with `caffeinate -s` for a session, or System
  Settings → Battery/Energy → prevent automatic sleeping when the display is off
  (mains power only). Closing the lid on battery sleeps regardless.
- **Resolve Tailscale CLI/daemon version skew before debugging `serve`.** A
  mismatch (for example CLI 1.96.4 against daemon 1.98.9) prints a warning on
  every single command. It is worth fixing first: `serve` semantics have shifted
  between versions, and the warning buries the real error in noise.
  `tailscale version` prints both halves; `brew upgrade tailscale` (or updating
  the app and reopening the shell) resolves it.
- **Suggest Add to Home Screen once HTTPS works.** On iOS: Safari → Share → Add
  to Home Screen. The user gets a full-screen icon with no browser chrome, plus
  web push. Worth offering proactively, because it is the payoff for setting up
  TLS and most users do not know to ask. Add the workspace URL
  (`https://<host>/<slug>/`) rather than the root if they live in one workspace.
  The home-screen app pairs separately from Safari: see § A phone installs
  before it pairs.
- **The URL to hand over.** The gateway root 307-redirects to the sole workspace
  or to the picker (`/~/`). A direct workspace link is
  `https://mymac.tailnet-name.ts.net/<slug>/`. Settings → Access prints exactly
  that under Connect URLs, with a Copy button, so point the user there rather
  than composing it by hand.

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
