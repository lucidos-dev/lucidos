---
name: Remote Access & HTTPS, Settings → Access (continued)
description: Part of Remote Access & HTTPS. Its index lists the sections it covers.
---

One part of `system-knowhow/remote-access`, which lists every part.

## Settings → Access (continued)

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
nothing if cancelled. **Cancel** stops the pending attempt. A mapping already
configured can remain; inspect `tailscale serve status`. Only one run exists
at a time: pressing Expose during one is refused. The button reads
"Setting up…" and stays disabled even if you navigate away and back, because
the run lives in the app, not the page.

If someone reports "Expose did nothing", the run may be waiting for them on
Tailscale's site, and the badge says so. A frozen window is a different bug:
every Mobile Access command runs on a worker thread, never the UI thread.

**Never run `/Applications/Tailscale.app/Contents/MacOS/Tailscale`.** It is the
GUI executable, not a CLI. Outside a GUI session it prints "The Tailscale GUI
failed to start ... (Tailscale.CLIError error 3)" and **exits 0**. So an
exit-code check reads success with unparseable output. That once made Mobile
Access show a Sign in button that reported success and changed nothing.

Use
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
- The **Other addresses Tailscale row** needs this gateway's own public health to
  answer at the candidate origin over validated TLS (§ The tailnet-status
  endpoint).
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

The engine and gateway share bind parsing in `crates/lucidos-core/src/net_config.rs`.

Direct access (Routes A and C, and any LAN or tailnet-IP URL) needs the gateway
bound beyond loopback. `tailscale serve` does not, since it proxies locally.

- Machine-global config lives in `network.toml`, beside the local token:
  `[gateway] bind = "loopback" | "all" | "<IP>"` plus `[engine] inherit`.
- **`[engine] inherit` reaches a directly-launched engine, not one the gateway
  spawned.** A gateway-spawned engine is pinned to loopback whatever the file
  says, since the gateway is the only door that authenticates.
- Test the workspace API through the gateway at `/<slug>/api/v1/` plus the
  endpoint path. A direct engine uses `/api/v1/` without the workspace prefix.
- Edit it from the **workspace picker's header gear → Network access** (the
  gateway bind). Or edit it per workspace in **Settings → Access → Network access** (the
  engine bind, when `inherit` is off).
- `./install.sh --bind all` writes the same file.
- Default is **loopback**, and a malformed value fails safe to loopback, never
  to all interfaces.
- A change takes effect only after a **restart** of the gateway. See
  [Restarting the gateway](#restarting-the-gateway).

### In a container (Kubernetes, Docker)

A Service or Ingress reaches the pod IP, never the pod's loopback. Each
listener has its own bind setting, and each defaults to loopback, so a desktop
install is unchanged. They take effect at gateway start.

| Listener | Port (packaged) | Bind setting |
|---|---|---|
| Main (UI and API) | `LUCIDOS_GATEWAY_PORT`, 5252 | `LUCIDOS_GATEWAY_BIND_ALL=1` |
| Public listener | `LUCIDOS_PUBLIC_PORT`, 5282 | `LUCIDOS_PUBLIC_BIND_ADDR=0.0.0.0` |
| Hook socket | `LUCIDOS_HOOK_PORT`, 5262 | `LUCIDOS_HOOK_BIND_ADDR=0.0.0.0` |

- `LUCIDOS_GATEWAY_BIND_ALL=1` binds `[::]`, dual-stack. On a node with IPv6
  switched off, set `LUCIDOS_GATEWAY_BIND_ADDR=0.0.0.0` instead.
- The two `*_BIND_ADDR` settings take an IP literal: `0.0.0.0`, `::`, or the
  pod IP. A value that is not an IP logs a line and stays on loopback.
- **Point a public Ingress at the public listener**, not the main port. Every
  request there counts as public, so a typed pairing code is refused.
- **Point a webhook Ingress at the hook socket only.** It answers one route and
  404s the rest, so a public path reaches no workspace.

> **Warning: a UI bound beyond loopback must sit behind an authenticating
> proxy.** Binding the main port or the public listener to `0.0.0.0` makes the
> whole gateway reachable from the pod network. Pairing is then the only gate.
> Put an identity proxy, proxy auth or mTLS in front, exactly as
> [Public only behind a front door you own](diagnose-first-never-assume.md#public-only-behind-a-front-door-you-own)
> requires. The hook socket is the one exception: every delivery authenticates
> by its own token or signature.

### Restarting the gateway

On an installed build the gateway is the always-on background service. How to
restart it depends on the install:

- **macOS app (DMG)**: in the **desktop app window**, press **Settings → System
  → Debugging → Restart engine**. There it restarts the whole service: the
  gateway and every workspace engine it spawned.
- **The same button in a browser or on a phone** restarts only that
  workspace's engine. The gateway keeps running, so a bind or TLS change does
  not apply.
- **Headless install**: restart the service from a terminal on the machine.
  On macOS run `launchctl kickstart -k gui/$(id -u)/com.lucidos.gateway.<slug>`.
  On Linux run `systemctl --user restart lucidos-gateway-<slug>.service`.
  `<slug>` is the instance name, `default` unless installed with `--name`.

Every route also restarts the engine you run in, so your turn ends. Tell the
user before, and let them start it.

**Cmd-Q does not restart it.** On the macOS app, Cmd-Q is *Close to Menu Bar*:
it hides the windows and stops nothing. Never tell the user to quit with Cmd-Q
and reopen to apply a bind or TLS change. *Quit and Stop Background Service*,
then reopening, also works, but halts triggers and sessions in between. Prefer
Restart engine.

After the restart, `GET /~/api/v1/health` lists any address the gateway still
awaits as `pending_binds`. Only a credentialed caller sees that field: send the
`x-lucidos-local-token` header with the contents of `~/.lucidos/private/local-token` (`~/.lucidos/local-token` in a home an older release created).
The agent sandbox hides that file, so an agent cannot make this call. Ask the
user to open the new address and say whether it loads.

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
  the local token file, such as the `lucidos` CLI.
- Local callers need no setup. The CLI, the gateway's proxy hop and the webhook
  hop already present their credential.

Two credentials exist, both minted at startup, both mode 0600.
`local-token` reaches everything, and
`hook-socket-token`, beside it, reaches only webhook delivery. So a
`tailscale funnel` on the hook port cannot restart a workspace.

**Reaching is not being you.** Every process on the machine can read the local
token, an agent's included. So an owner-only act it asks for waits for
**Allow once** on a card, answered from a paired device. Apply, a human-only
setting and starting a coding agent are such acts. Only the gateway's signed word that a
paired device sent a request proves it came from you.

Binding to the tailnet IP itself (`100.x.y.z`) is the middle ground: the
tailnet reaches it, the coffee-shop LAN does not.

**A configured IP that is not up yet does not hold the start back.** An `<IP>`
bind always comes with loopback, and only loopback is required. The gateway
serves on loopback at once, retries the configured address in the background,
and listens there once the interface appears. This matters at boot: launchd
starts the service before `tailscaled` assigns the `100.x` address, so that bind
fails with `Can't assign requested address`.

So right after a restart, a `100.x` URL can be briefly unreachable while the
local one works. `GET /~/api/v1/health`, sent with the local token, lists
addresses still awaited as `pending_binds`, empty once all are bound. Without a
credential it answers only `status`, `role`, `instance_id`, `gateway_protocol`
and `tls`, so a stranger on the network learns no release or address. `loopback` and `all` are unaffected:
their single address is required, and failing to bind it is a hard error
(the port is held, not an interface missing).

## Connecting accounts from another device

Connecting an OAuth account (Google, Microsoft, Dropbox and so on) sends the
browser to a redirect URI after consent. Where that browser runs decides which
URI works. The rules live in `oauth-providers.md` § Web redirect URI.

- **A browser on a computer running Lucidos** needs no setup, even for a remote
  workspace opened through that computer's gateway. The local gateway catches
  the loopback redirect and passes it to the remote gateway.
- **A phone, an iPad, or a laptop opening a remote address** needs two things.
  The gateway address must be https (one of the routes above). The credential
  needs a registered **web redirect URI** on a Web client, with its secret.
  Without them, Connect refuses at once and names the fix.
- **An SSH port-forward to a remote gateway is unsupported.** The browser looks
  local to the remote gateway, which then offers a loopback redirect that lands
  on the wrong computer. Open the remote workspace through this computer's own
  Lucidos instead.

### Sign in with ChatGPT on a headless workspace

The *ChatGPT plan provider* accepts only a loopback redirect,
`http://127.0.0.1:14981/oauth/callback`. So the browser must run on a computer
with a Lucidos gateway. No https route or web redirect URI changes that.

- **A headless server or cloud workspace:** open it through the Lucidos on your
  laptop, then press **Continue with ChatGPT** in Settings → Providers. The
  laptop's gateway catches the redirect and passes it to the remote
  workspace, which signs in as itself.
- **A server with its own desktop browser:** run `lucidos chatgpt-plan sign-in`
  there, and open the page it prints in that browser.
- **A phone, or a browser opening a remote address directly:** refused before
  any page opens. Use one of the two ways above.

### Claude Code's MCP login is a different flow

A Claude Code session that logs in to an MCP server runs the `claude` CLI's own
OAuth flow. That flow binds a random loopback port on the computer running
Claude Code, and Lucidos never sees it. So a browser on another computer cannot
finish it. Two ways out:

1. Run the MCP login in a browser on the computer that runs Claude Code.
2. Or SSH-forward that random port from the browser's computer, then open the
   login link there: `ssh -L <port>:127.0.0.1:<port> <host>`. The port is in
   the redirect URL the login link carries.

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
  that under **Other addresses** (a fold under Pair a device), with a Copy
  button, so point the user there.

## Quick triage

| Symptom | First thing to check |
|---|---|
| "Not Secure" label | Route A is in use. Move to B, or C. |
| Phone loads nothing at all | Bind (loopback only), host asleep, or the device is not on the tailnet. |
| Certificate error naming a different host | SAN list is missing the MagicDNS name. `openssl x509 ... -text`. |
| "Not trusted" on one device only | iOS trust step 4 (Certificate Trust Settings) was skipped. |
| `500 ... your Tailscale account does not support getting TLS certs` | The account-level HTTPS toggle is off. Nothing local will fix it. |
| `tailscale serve` prints a `login.tailscale.com/f/serve` link and never returns | Serve is not enabled for the tailnet. Open that exact link and approve; the command finishes by itself. Not a hang. |
| `error enabling https feature: error 501 Not Implemented` | The control server (Headscale, or another self-hosted one) has no feature-enable API and issues no certificates. Route B cannot work there. Use Route A, or C with your own certificate. |
| Expose reports a CLI syntax change on a current CLI | Read past it. That line comes from the pre-1.52 fallback attempt, which now runs only on a rejected flag; if you see it on 1.52+, the build predates that gate. |
| Works on 5252, fails on 5251 (or vice versa) | Two gateways, two TLS setups. Probe the failing port on its own. |
| No push notifications | Not a secure origin (check that first), or the OS-level permission was never granted. |
| Apps load blank behind a reverse proxy | A path prefix was added. Serve Lucidos at the origin root, on its own port. |
| `tailscale serve status` says "No serve config" but the URL works | Route C is in play. Not a problem. Probe the port. |
| Anything needing a live socket fails, while pages load | A hop is dropping the WebSocket upgrade. Probe `/<slug>/api/v1/ws-echo`, which upgrades and echoes what you send it. A `101` means every hop carried it. |
| Account Connect refuses on a phone, asking for https or a web redirect URI | The browser is not on a computer running Lucidos. See § Connecting accounts from another device. |

## Diagnostic evidence

Gateway and engine diagnostics use separate sanitized sinks.
Diagnostic collection uses registry-derived paths rather than scanning service or host logs.

Follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access and the raw-host-log restriction.
