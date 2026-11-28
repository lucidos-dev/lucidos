---
name: Lucidos CLI (lucidos), Subcommands (continued)
description: Part of Lucidos CLI (lucidos). Its index lists the sections it covers.
---

One part of `system-knowhow/lucidos-cli`, which lists every part.

## Subcommands (continued)

### `lucidos chatgpt-plan status [--json]` / `sign-in` / `sign-out [--forget-account]`

Sign Lucidos in with ChatGPT, for the *ChatGPT plan provider*: GPT models on
the user's own ChatGPT subscription. Settings → Providers does the
same through the same routes, so a sign-in started here shows there too.

```bash
lucidos chatgpt-plan sign-in
# Open this page in a browser on this computer:
# https://auth.openai.com/oauth/authorize?...
# Waiting for ChatGPT. Press Ctrl-C to stop waiting; the sign-in keeps running.
# Signed in with ChatGPT as user@example.com.

lucidos chatgpt-plan status
# Signed in with ChatGPT as user@example.com.

lucidos chatgpt-plan sign-out
# Signed out of ChatGPT.

lucidos chatgpt-plan sign-out --forget-account
# Forgot the ChatGPT account. Any ChatGPT account may sign in next.
```

- Open the page in a browser on the computer running Lucidos. ChatGPT sends
  the browser back to `http://127.0.0.1:14981/oauth/callback`, which only that
  computer's Lucidos gateway answers. A browser on another device is refused
  before any page is made. An engine started without the gateway refuses too.
- The sign-in waits ten minutes for the browser to come back.
- The engine keeps the tokens and refreshes them. No command prints one.
- `status --json` prints the engine's own answer from
  `GET /api/v1/providers/chatgpt-plan`: `account` (`email`, `signed_in_at`),
  `sign_in`, `sign_in_again`, `registered`, `paused_until` and
  `pause_remaining_ms`.
- `sign_in_again` is true when OpenAI refused the stored sign-in and none has
  been made since. Run `sign-in` again.
- `paused_until` is set while the plan's usage limit pauses plan requests.
  Lucidos sends none until then, and never moves the call to the `openai`
  route.
- `sign-out` also asks OpenAI to revoke the refresh token, best effort.
- After the first sign-in, only that ChatGPT account may sign in again, and
  `registered` is true. `sign-out --forget-account` forgets the account, so
  the next sign-in registers Lucidos with OpenAI afresh and may use any account.
  A usage limit set for the old registration in ChatGPT does not carry over.
  Settings → Providers offers the same as **Use another account**.
- Starting is `POST /api/v1/providers/chatgpt-plan/sign-in`, which answers
  `authorize_url` and `expires_at`. Stopping is
  `POST /api/v1/providers/chatgpt-plan/sign-out`, with `?forget_account=true`
  to forget the account. It answers `signed_out` and `account_forgotten`.
  Both need a person's
  credential, and an app cannot reach either.

### `lucidos pair` (mint a code that lets a device in)

Lucidos authenticates every caller that reaches it over the network. A device is
paired once and then remembered, so a stranger who reaches the port is refused.

```bash
lucidos pair                        # print a code, a secret, and where to use them.
lucidos pair --qr                   # draw it as a QR to scan.
lucidos pair --host mac.ts.net      # pick the hostname the QR points at.
lucidos pair --label "My iPhone"    # name the device in the paired list.
lucidos pair --port 5300            # a gateway on an unusual port.
lucidos pair --gateway              # a secret for another gateway instead.
lucidos pair --gateway --manage     # ...that may also manage users and devices here.
lucidos pair --gateway --control    # ...that may also create and restore workspaces here.
lucidos pair --gateway --admin      # ...that may manage this gateway as a device paired here.
```

Run it in a terminal on the machine Lucidos runs on, then type the code into the
device you want to let in. It works once and expires in five minutes.

**The typed code works only from another device on the LAN or the tailnet.** On
this machine, and through a tunnel or proxy, Lucidos refuses it (ADR 0615).
On the new device, open the printed link. You can also paste the link or
*pairing secret* into **Pairing secret or link**.
New device links carry credentials in `/~/#pair=` fragments. Ordinary printed links prefer the configured external address, with loopback as fallback.

**`--gateway` lets another gateway in, not a device.** It mints a *gateway
pairing secret*, which never pairs a device (ADR 0590). Paste it on the other
gateway under **Settings → Access**, in **Add a remote gateway**, with the
`https` address this command prints. Gateways pair over `https` only, so with no such
address it says what to set up: `tailscale serve`, or an external address.
`--label` names the other gateway in this one's paired list.

`--qr` and
`--host` do not combine with it. `--manage` starts the pairing with the
*manage grant* on, so devices on the other gateway may manage this gateway's
users and devices (ADR 0656). `--control` starts it with the *control grant*
on, so they may create, start, stop and restore this gateway's workspaces (ADR
0811). `--admin` starts it with the *admin grant* on, which includes both.
Without a flag its grant is off, and the Paired gateways row turns it on later.

**With two gateways running it refuses rather than guessing.** A device pairs to
a gateway, so a code only works on the one that minted it (ADR 0132). This
command finds a gateway by probing 5252 then 5251, which is the packaged app
then a dev checkout. Both answering is ambiguous, so it stops and names the
ports. Pass `--port` to say which one the new device will reach.

**Reach for it only when nothing is paired yet, and know where it lives.** The
desktop app pairs its own window on launch. Any paired device can add the next
one from **Settings → Access → Pair a device**. That mints the same code and
shows the same QR, with no terminal step. `lucidos` is on no `PATH` either. A
desktop install keeps it at `Lucidos.app/Contents/Resources/lucidos`, and a
headless one under the install prefix in `runtime/current/`.

**`--qr` needs an address the phone can reach.** This command talks to
`127.0.0.1`, and a QR aimed there helps nobody. It takes the first of these:

1. `--host`, which implies `--qr`. It is never refused: probing only picks which
   of its two origins to use.
2. The gateway's *external address* (`lucidos external-address`, below), used as
   set, since the user typed it.
3. The gateway's own HTTPS origin, when it serves a *managed certificate*
   (`lucidos tls status`, below) and that origin answers as this gateway.
4. The tailnet origin Tailscale itself reports: the `https://` name when
   Tailscale can certify it, then the MagicDNS name, then the tailnet address.
   When the machine is not on a tailnet, it prints Tailscale's reason.

That is the Access page's order too, minus the two steps a terminal cannot see:
the reader's own address and the LAN address. Tailscale only picks a name to
print, and the auth decision reads none of it.

**It knocks on the tailnet door.** Holding a tailnet address does not make it
reachable. The packaged gateway binds loopback, where `<name>:<port>` is dead.
`tailscale serve` fronts 443 on that same name and answers where the gateway's
own port does not. So each tailnet origin is probed, and the first that answers
with THIS gateway's health `instance_id` wins. With none answering there is no
QR, and the command says which knob to turn.

The QR is drawn black on white, because a dark terminal would invert it and
many scanners refuse that. `NO_COLOR` drops the escapes.

**A browser has to pair too, even on that same machine.** Proving you are local
means reading a file only your user can read, and a browser cannot read files.
So a browser on the host uses the same code as a phone.

Only a process on that machine, or an already-paired device, can mint a code.
That stops a remote caller from pairing itself in.

**Once any device is paired, this command's code waits for one.** Every process
on the machine can read the local token, an agent's included. So the device
that redeems the code waits on its pairing screen until someone taps **Allow**
under **Settings → Access** on a device already paired. **Don't allow** fails
it. The command prints this when it applies, and so does `--gateway`.

A code minted on a paired device never waits. With no device paired, nothing
waits, as on a fresh install.

Nothing you run from a coding-agent session, a trigger or a script needs this.
Those already prove they are local, and the CLI attaches that proof itself.

### `lucidos devices list | revoke <id> | revoke --all`

Lists or revokes the devices paired with this machine's gateway, through the
local token. `--port` names the gateway when two are running.

```bash
lucidos devices list            # id, name and last seen, one per line.
lucidos devices revoke <id>     # revoke one; it must pair again.
lucidos devices revoke --all    # revoke every paired device.
```

**`revoke --all` is the way back in when no paired device works.** While any
device is paired, a `lucidos pair` code waits for a paired device's Allow. If
the only one lost its credential, such as a desktop window whose cookie was
cleared, nobody can answer. Revoke the stale rows, then pair again.

### `lucidos external-address show | set <origin> | clear`

The *external address* is the origin other devices open to reach this gateway,
through a route you run: a reverse proxy, WireGuard, a company VPN, or a
company name with its own certificate. Pairing QRs use it ahead of Tailscale.

```bash
lucidos external-address show                 # the address, who set it, this machine's addresses.
lucidos external-address set https://lucidos.example.com
lucidos external-address clear                # QRs go back to the tailnet.
lucidos external-address set --port 5300 http://10.8.0.2:5300   # one gateway of two.
```

- **One per gateway.** It lives in `~/.lucidos/private/network.toml` (`~/.lucidos/network.toml` in a home an older release created) under the
  gateway's port, so two gateways on one machine never share one. With two
  running, the command refuses until `--port` names one, as `lucidos pair` does.
- **An origin only:** scheme, host and optional port, with no path. A loopback,
  link-local or unspecified host is refused, since no other device can open it.
- **`set` is probed.** The gateway fetches `<origin>/~/api/v1/health` and
  compares its per-boot `instance_id`. A match is verified. No answer from here
  saves with a warning, since hairpin NAT and client-only VPNs are ordinary. An
  answer it cannot compare, such as an identity proxy's login page, saves with
  a warning too. Only a different gateway answering refuses the save.
- **The environment wins.** `LUCIDOS_EXTERNAL_ADDRESS` in the gateway's
  environment overrides the file and refuses every write. `show` says so.
- **Four editors, one value:** this command, **Settings → Access → External
  address**, `install.sh --external-address`, and
  `PUT /~/api/v1/control/external-address`.

### `lucidos tls trust | forget`

Acts on the *local certificate*, which serves `https://localhost` on this
machine (`system-knowhow/remote-access/the-three-routes.md` § The local certificate). It runs as you, since only
you can change your own certificate store.

```bash
lucidos tls trust    # trust it, so this machine's browsers open https://localhost.
lucidos tls forget   # remove it and its trust, for an uninstall.
```

- **`trust` makes the certificate first** when no gateway has yet. On macOS it
  asks for your password, for the login keychain. On Linux it needs `certutil`
  and names the package when it is missing.
- **`forget` reaches every install of yours.** The next gateway start makes a
  new one, which needs `trust` again. `uninstall.sh --all --purge` runs it,
  unless the macOS app is still installed.

### `lucidos tls status [--port <P>]`

Says whether this user trusts the local certificate, then whether other devices
reach this gateway over HTTPS, and why not when they do not. It only reads: the
gateway settles its certificate itself (ADR 0600).

```bash
lucidos tls status              # local trust, the source, the https address, the expiry.
lucidos tls status --port 5300  # one gateway of two.
```

- **Three answers.** Your own certificate (`LUCIDOS_TLS_CERT`), a *managed
  certificate* from your tailnet with the address to open, or none with the
  reason. A failed update says so, while the current certificate still serves.
- **Both at once.** Your own certificate may not satisfy a phone on the tailnet
  name. Then that name gets a managed certificate, and every other name keeps
  yours. The output says which serves what. When the tailnet name gets neither,
  it says why.
- **Local only.** It reads `GET /~/api/v1/control/tls` with the local token, so
  run it on the gateway's machine. A paired device sees the same on Settings →
  Access.
- **With two gateways running it refuses** until `--port` names one, as
  `lucidos pair` does. Each gateway has its own certificate.
- **No HTTPS?** See `system-knowhow/remote-access/the-three-routes.md` § Automatic: the gateway's own certificate,
  for what a tailnet admin turns on.

### `lucidos update-channel show | list | set | add | remove`

An **update channel** selects where update checks look. Named custom slots keep
their addresses when you switch channels.

```bash
lucidos update-channel show
lucidos update-channel list
lucidos update-channel set stable
lucidos update-channel set beta
lucidos update-channel add Preview https://example.com/latest.json --use
lucidos update-channel set custom:Preview
lucidos update-channel set stable
lucidos update-channel remove Preview
lucidos update-channel show --port 5300
```

- **Persistent selection.** The gateway stores the channel and custom slots in
  `~/.lucidos/updates.toml`. Switching to Stable keeps all slots.
- **Custom slots.** `add <name> <url>` creates or replaces one slot.
  `--use` selects it in the same write. `remove <name>` refuses a selected
  slot; select another channel first.
- **The rule:** `https`, or `http` only to `localhost`, `127.0.0.1` or `[::1]`.
  Names and URLs are validated before any gateway request.
- **Availability.** `list` marks the selection and names unavailable channels'
  reasons. Beta and Local require the packaged macOS app.
  Nightly is reserved and refused. Headless installs support Stable and Custom.

  Local also requires staged data. It reads the gateway's https loopback
  origin, whatever TLS the gateway serves other devices.
- **Check results.** `show` names the channel, resolved address, channel version,
  check error, and Local staging directory. An ahead notice names both versions.
  A broken feed fails closed; no update ever downgrades the running build.
- **Signatures still count.** The app checks every bundle against its baked key.
- **Command replacement.** `update-channel` replaces `update-address` without an alias.
  Use `set stable` in place of `reset`.

The file is shared by every gateway on the machine. A channel write accepts
Beta or Local when this install supports it or the machine has a packaged app.
With a readable machine-local token, the command contacts the gateway directly
and can probe packaged and dev ports, respecting the dev override.

Inside an agent without that token, it uses the engine's gateway control request
route. Reads need no card. A write prints `request_id: <id>` and exits 3 when
the owner must decide. Ask the question tool with that id as the only question.

After Allow once, rerun the same command. The approval covers that exact request
once, on this thread. `--port` is refused on the engine path, which only reaches
its own gateway. Without a token or an agent environment, the command refuses.

The agent route is `POST /api/v1/engine/gateway-control`, with `method`, `path`,
`body` and a change's `reason`. It accepts only `GET /gateway/status` and
`PUT /release-check`. Paths are relative to the gateway control API.
An answer contains `response`, or `request_id` when awaiting the owner.
Pairing, device-session and broker routes are unreachable through this route.

`PUT /~/api/v1/control/release-check` edits the same selection with `channel`,
`custom_upsert` (`name`, `url`), and `custom_remove`.
Custom selections use `custom:<name>`. Invalid writes return 400 without saving.

The gateway status reports `channel`, `channels` with availability reasons,
`custom`, `channel_version`, `ahead_of_channel`, and `channel_error`.
A broken feed offers nothing and reports its error.
`update_address` remains the resolved app endpoint, or null for Stable.

`local_channel_dir` names the staging directory under the gateway's home.
Local reads `latest.json` and its one `darwin-aarch64` bundle there.
Its feed and bundle routes require loopback and the machine-local token.
The gateway resolves their URLs from its current origin, without storing a port.

