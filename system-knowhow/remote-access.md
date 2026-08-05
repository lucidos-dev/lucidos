---
name: Remote Access & HTTPS
description: Use when the user wants to reach Lucidos from a phone, tablet, or another machine: "access from my phone", "remote access", "Mobile Access", "Expose", "Tailscale", "HTTPS", "not secure warning", "add to home screen", "certificate", "mkcert", "tailscale serve", "Serve is not enabled on your tailnet", "reverse proxy". Covers the Settings > Access page and what each of its controls does, the Expose run and the tailnet approval it can wait on, finding which gateway is listening on which port, the three routes to HTTPS (tailscale serve, mkcert, plain HTTP over the tunnel), and the per-device certificate trust steps.
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

Both sections render **everywhere**, phone browsers and the installed PWA
included. What varies by platform is how much each can say, never whether it
appears. Only the **actions** are gated: Connect URLs, Sign in to Tailscale and
Expose are native commands with no HTTP equivalent, so they exist on the
packaged desktop app alone. **Get Tailscale** is a link rather than a bridge
call, so it is offered wherever it can be acted on, and it opens the App Store
on iOS, the Play Store on Android, and `tailscale.com/download` otherwise.

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

**Connect URLs** lists the addresses the engine answers on:

- **This Mac**: `http://localhost:<port>`. A secure origin, so a full PWA works
  here, which is Route D.
- **Local network**: shown only when the gateway is bound beyond loopback. Plain
  HTTP, so no PWA install and no push. Bound to loopback (the packaged default)
  the row points at the **Network access** section further down this same page
  instead of printing a dead URL.
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

The Access page now follows this rule rather than guessing: it publishes
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
  gateway bind), or per workspace in **Settings → Access → Network access** (the
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
| `tailscale serve` prints a `login.tailscale.com/f/serve` link and never returns | Serve is not enabled for the tailnet. Open that exact link and approve; the command finishes by itself. Not a hang. |
| Expose reports a CLI syntax change on a current CLI | Read past it. That line comes from the pre-1.52 fallback attempt, which now runs only on a rejected flag; if you see it on 1.52+, the build predates that gate. |
| Works on 5252, fails on 5251 (or vice versa) | Two gateways, two TLS setups. Probe the failing port on its own. |
| No push notifications | Not a secure origin (check that first), or the OS-level permission was never granted. |
| Apps load blank behind a reverse proxy | A path prefix was added. Serve Lucidos at the origin root, on its own port. |
| `tailscale serve status` says "No serve config" but the URL works | Route C is in play. Not a problem. Probe the port. |
