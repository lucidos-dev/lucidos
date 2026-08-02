---
name: Remote Access & HTTPS
description: Use when the user wants to reach Lucidos from a phone, tablet, or another machine: "access from my phone", "remote access", "Mobile Access", "Tailscale", "HTTPS", "not secure warning", "add to home screen", "certificate", "mkcert", "tailscale serve", "reverse proxy". Covers the Settings > Mobile Access page and what each of its controls does, finding which gateway is listening on which port, the three routes to HTTPS (tailscale serve, mkcert, plain HTTP over the tunnel), and the per-device certificate trust steps.
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
`lucidos-engine` per running workspace on its own port. On a packaged install
the engines are loopback-only and the gateway is the sole network-facing
surface; on a dev checkout the engines can be network-facing too. Either way the
**gateway** port is the one to hand a remote device, because it routes to every
workspace by slug.

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

## Never expose Lucidos to the open internet

Lucidos has **no inbound API authentication**. Anything that can reach the port
acts as the user, with full access to every workspace, every credential, and
every coding-agent capability. So:

- Use `tailscale serve` (tailnet-private), **never `tailscale funnel`** (public).
- No router port-forward, no public reverse proxy, no ngrok-style tunnel.
- The tailnet is the authentication boundary. Devices join the tailnet; that is
  how they are authorized.

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

**Prerequisite the agent cannot satisfy.** Tailnet HTTPS is an **account-level**
toggle: <https://login.tailscale.com/admin/dns> → **HTTPS Certificates** →
**Enable HTTPS**. Only a tailnet admin can flip it, in a browser. If it is off,
cert provisioning fails with exactly this:

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

The packaged desktop app does this for the user: **Settings → Mobile Access**
runs `tailscale serve --bg https / http://127.0.0.1:<port>` behind its **Expose**
button and then shows the resulting `https://mymac.tailnet-name.ts.net` URL.
Prefer pointing the user there over hand-running commands when they are on the
packaged app. See § Settings → Mobile Access below for what the page can and
cannot do.

`serve` flag syntax has changed across CLI versions (the positional
`https / <target>` form and the `--https=<port> <target>` form both exist in the
wild). `tailscale serve --help` is the authority for the installed version, and
`tailscale serve status` is the proof of what actually got configured.

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

## Settings → Mobile Access

The page that drives all of this on the packaged desktop app. Point the user
here before hand-running commands.

**It has two halves, split by whether they need the Mac.**

| Half | Contains | Shown |
|---|---|---|
| Machine-side | Connect URLs, Sign in to Tailscale, Expose | Packaged desktop app only |
| Install | What Tailscale buys you, Get Tailscale, the phone steps | Everywhere, phone browsers and the PWA included |

The machine-side controls are native commands with no HTTP equivalent, so they
cannot work in a browser. The install half needs nothing, and the phone is
exactly where it is worth reading, so the page is reachable from a phone and
rewords itself for whichever device opened it. **Get Tailscale** opens the App
Store on iOS, the Play Store on Android, and `tailscale.com/download` otherwise.

**Connect URLs** lists the addresses the engine answers on:

- **This Mac**: `http://localhost:<port>`. A secure origin, so a full PWA works
  here, which is Route D.
- **Local network**: shown only when the gateway is bound beyond loopback. Plain
  HTTP, so no PWA install and no push. Bound to loopback (the packaged default)
  the row points at Settings → Network access instead of printing a dead URL.
- **Tailscale**: the tailnet IP over plain HTTP until `serve` is configured, and
  the `https://...ts.net` URL once it is. See the detection trap below for why
  the HTTPS one appears only after serving is proven.

Both plain-HTTP rows obey the network bind, and for the same reason: being on a
tailnet does not mean the gateway is **listening** on the tailnet address. Under
the packaged loopback default neither prints, because both URLs would be dead.
A bind pinned to the tailnet address shows the Tailscale row and reports the LAN
as off, which is accurate: that bind serves one address, and it is not a LAN one.
`serve` is unaffected by all of this, since it proxies from this machine to
`127.0.0.1` and needs no wider bind.

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

**The four states of the Tailscale section**, from those two independent facts:

| Tailnet state | CLI | The page shows |
|---|---|---|
| Tailscale absent | any | **Get Tailscale** |
| Installed, not on a tailnet | yes | **Sign in**, with an optional auth key |
| Installed, not on a tailnet | no | Sign in from the Tailscale menu-bar app |
| On a tailnet, not serving | yes | **Expose** |
| On a tailnet, not serving | no | How to get the CLI; the plain-HTTP URL works meanwhile |
| On a tailnet, serving | any | The `https://...ts.net` URL, plus **Re-apply** with a CLI |

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

The Mobile Access page now follows this rule rather than guessing: it publishes
the `https://...ts.net` URL only after probing **port 443 on the tailnet
address** and finding something answering. Before that it shows the plain-HTTP
tailnet URL, which is honest about what works today. It previously showed the
HTTPS URL the moment a MagicDNS name resolved, which is well before anything is
listening on it.

It deliberately does **not** ask `tailscale serve status` instead. That answers
"does *a* serve mapping exist", which is a different question: with only the
8443 mapping from the two-gateway setup above, the config is non-empty while
`https://<name>` on 443 is still dead. The page publishes exactly one URL, so it
tests exactly that one. Note the probe proves a **listener**, not a working
certificate, which is the one case where the URL can appear before it loads.

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
- Edit it from the **workspace picker → Settings → Network access** (the
  gateway bind), or per workspace in **Settings → System → Network access** (the
  engine bind, when `inherit` is off).
- `./install.sh --bind all` writes the same file.
- Default is **loopback**, and a malformed value fails safe to loopback, never
  to all interfaces.
- A change takes effect only after a **restart**.

Binding to the tailnet IP specifically (`100.x.y.z`) is the middle ground: the
tailnet reaches it, the coffee-shop LAN does not.

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
- **The URL to hand over.** The gateway root 307-redirects to the sole workspace
  or to the picker (`/~/`). A direct workspace link is
  `https://mymac.tailnet-name.ts.net/<slug>/`.

## Quick triage

| Symptom | First thing to check |
|---|---|
| "Not Secure" label | Route A is in use. Move to B, or C. |
| Phone loads nothing at all | Bind (loopback only), host asleep, or the device is not on the tailnet. |
| Certificate error naming a different host | SAN list is missing the MagicDNS name. `openssl x509 ... -text`. |
| "Not trusted" on one device only | iOS trust step 4 (Certificate Trust Settings) was skipped. |
| `500 ... your Tailscale account does not support getting TLS certs` | The account-level HTTPS toggle is off. Nothing local will fix it. |
| Works on 5252, fails on 5251 (or vice versa) | Two gateways, two TLS setups. Probe the failing port on its own. |
| No push notifications | Not a secure origin (check that first), or the OS-level permission was never granted. |
| Apps load blank behind a reverse proxy | A path prefix was added. Serve Lucidos at the origin root, on its own port. |
| `tailscale serve status` says "No serve config" but the URL works | Route C is in play. Not a problem. Probe the port. |
