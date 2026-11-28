---
name: Remote Access & HTTPS, Diagnose first, never assume
description: Part of Remote Access & HTTPS. Its index lists the sections it covers.
---

One part of `system-knowhow/remote-access`, which lists every part.

## Diagnose first, never assume

**A machine often runs more than one Lucidos gateway, with different TLS
setups for other devices.** The usual pair is the packaged `Lucidos.app` on
**5252** and a dev gateway from a source checkout on **5251**. The dev one
serves other devices **https** whenever the checkout has `.certs/cert.pem` +
`.certs/key.pem`. Advice for one is wrong for the other, so probe before you say
anything. On `localhost` both serve https with the local certificate (§ The
local certificate).

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

Expect a gateway listener for each running install. Gateway-managed engines
bind loopback in both packaged and dev setups. A directly launched engine can
use a wider bind, but then requires a local credential. See § An engine that
faces a network asks for a credential. Inspect its launch command and bind settings.
Hand remote devices the gateway port, which routes workspaces by slug.

### Step 2: which port speaks TLS to other devices

**Never probe `127.0.0.1` for this.** Loopback always answers https with the
local certificate, and plain HTTP there gets a `301` to `https://localhost`. So
a loopback probe says nothing about what a phone gets. Probe the address the
device uses instead, its LAN IP or tailnet name (`<address>` below):

```bash
for p in 5251 5252; do
  printf 'port %s  https=%s  http=%s\n' "$p" \
    "$(curl -sk -o /dev/null -w '%{http_code}' --max-time 3 https://<address>:$p/)" \
    "$(curl -s  -o /dev/null -w '%{http_code}' --max-time 3 http://<address>:$p/)"
done
```

| Reading | Conclusion |
|---|---|
| `https=307`, `http=000` | that port terminates TLS for this address |
| `https=307`, `http=308` | TLS for this address, and plain HTTP redirects to the certificate's own origin |
| `http=307`, `https=000` | that port is plain HTTP for this address |
| `307` on either | **healthy.** The gateway redirects `/` into a workspace path (`/<slug>/`) or the picker (`/~/`). A 307 is the normal answer, not an error. |
| `200` | also fine (a direct workspace or picker URL) |
| `000` on both | nothing listening there, or a firewall ate it |

`lucidos tls status` answers the same question without a probe.

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

Any paired device can then let the next one in. **Settings → Access → Pair a
device** mints a code and draws a QR the phone can scan.

**Once a device is paired, a local process's code waits for it.** Every process
on the machine can read the local token, so the token alone no longer lets a
device in. A code from `lucidos pair`, or the desktop window's own, redeems only
after a paired device taps **Allow** under **Settings → Access**. The waiting
device says so on its pairing screen. Every paired device gets a notification
that opens that page, where **Waiting for your Allow** lists the request.
**Don't allow** fails it.

A code a paired device minted never waits. When no paired device works any
more, `lucidos devices revoke --all` clears the stale rows, and the next code
pairs at once. Never move, rename or edit the gateway's own files to skip the
Allow: these two paths cover every case.

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

### Public only behind a front door you own

Authentication raises the floor. It does not make a bare public origin a good
idea: pairing is then the only gate. A typed *pairing code* is refused from a
public address, and the long *pairing secret* in the QR is the only way in
(ADR 0615).

- Use `tailscale serve` (tailnet-private), **never `tailscale funnel`** on the
  gateway's own port.
- No bare router port-forward and no ngrok-style tunnel straight at the port.
- A public address needs an outer gate you own: an identity proxy, proxy auth
  or mTLS, in front of a reverse proxy. Pairing stays the inner gate. Set that
  address as the external address (§ Any other VPN).
- Point a tunnel or reverse proxy on this machine at the *public listener*
  (5282 packaged, 5281 dev), not the main port. Every request there counts as
  public, whatever headers it carries.
- A tailnet or a private VPN is the simpler outer boundary.

### The one exception: a webhook's own socket

A *webhook* is the one surface meant for callers who will never join your
tailnet: GitHub cannot pair. Webhook deliveries answer on their own port, the
*hook socket*. Point any public webhook route exclusively at this socket
(ADR 0097).

**The isolation is structural.** The hook socket has one delivery route,
`POST /<slug>/<webhook-id>`, and answers 404 to everything else, a wrong method
included. This keeps the control plane and workspace UI outside the public
webhook route.

The hook port is the gateway's plus ten: **5261** in dev, **5262** packaged.
`LUCIDOS_HOOK_PORT` overrides it, and `0` switches the socket off.
It binds loopback unless `LUCIDOS_HOOK_BIND_ADDR` says otherwise (§ In a
container).

#### Tailscale Funnel

```bash
tailscale funnel --bg 5262   # packaged: publish ONLY the hook socket
tailscale funnel status      # what is public right now
tailscale funnel off         # stop publishing
```

`tailscale funnel --help` is the authority for the installed version.
Deliveries arrive at `https://<machine>.<tailnet>.ts.net/<slug>/<webhook-id>`.

#### Cloudflare Tunnel

Run `cloudflared` on the gateway host. Add a published application with a
dedicated public hostname such as `hooks.example.com` and the origin service
`http://127.0.0.1:5262` for a packaged install. Use the actual hook port.
[Cloudflare Tunnel setup](https://developers.cloudflare.com/tunnel/get-started/)

#### Reverse proxy over the tailnet

On the gateway host, set `LUCIDOS_HOOK_BIND_ADDR` to that host's actual
Tailscale IP literal. On the proxy, serve a public HTTPS hostname and forward
its webhook requests to that IP and the hook port over the tailnet. Only the
proxy needs to reach the hook port. Keep the backend connection on the tailnet.

#### Set the public webhook URL

Open **Settings → Webhooks** in each workspace that uses the route. Enter the
HTTPS base URL in **Public webhook URL**. Lucidos uses
this URL ahead of Funnel discovery. Copy the delivery URL shown for each
webhook into its sender's configuration.

If you publish on a path prefix such as `https://example.com/hooks`, configure
the public route to strip `/hooks` before forwarding
`/<slug>/<webhook-id>` to the hook socket. A dedicated hostname can forward
the delivery path as-is.
[Cloudflare Tunnel path forwarding](https://developers.cloudflare.com/tunnel/features/locally-managed-tunnels/configuration-file/#how-traffic-is-matched)

**Every delivery still authenticates** by bearer token or by the sender's
own signature. Each webhook emits one pinned event, fixed at creation. Create
one with `lucidos webhooks create`. `system-knowhow/lucidos-cli.md` covers the
tokens and the GitHub, Slack and Stripe signature shapes.

**Senders can redeliver.** By default each arrival emits again. `--dedupe`
names the header carrying the sender's delivery id and collapses the repeat.
The CLI page covers it, and the `--headers` allow-list that puts a chosen
request header in the payload.

**Lucidos watches this path, and only watches.** Every 15 minutes it knocks on
each public address for an enabled webhook and expects a 401 from the hook
verifier. With a public webhook URL set, it probes that route. Otherwise it
reads `tailscale serve status --json` to discover a Funnel serving the hook
port. Lucidos leaves all tunnel and proxy configuration to you.

It catches one address family going dark while the other still works. The
outage shows as a bar across the app and a line on the row in **Settings →
Webhooks**. Lucidos emits `WebhookIngressDegraded` and `WebhookIngressRecovered`
once each per outage, for a *trigger* to act on.

Lucidos may have a running hook socket and an enabled webhook but no public
route to probe. In that case, **Public webhook ingress is unverified** appears
in the app bar and on enabled rows. Set the public webhook URL if you
already run a public route. If you set a URL and still see the notice after
the next check, confirm its hostname resolves to a public address.

**Sleep pauses outage detection.** A round the machine slept through, or the
first one after a wake, never counts toward an outage. A sleep of 30 minutes
or more emits `WebhookDeliveriesSleptThrough` once, with `slept_secs`. GitHub
does not resend a failed delivery on its own, so a trigger on it can tell you
to check the sender's delivery log.

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

