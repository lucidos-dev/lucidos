---
name: Remote Access & HTTPS, Settings → Access (continued)
description: Part of Remote Access & HTTPS. Its index lists the sections it covers.
---

One part of `system-knowhow/remote-access`, which lists every part.

## Settings → Access (continued)

### Pair a gateway

A *gateway pairing* lets one gateway open another gateway's workspaces, for
every device paired with it (ADR 0590). Two sections on **Settings → Access**
carry it, right under **Pair a device**:

| Section | On which gateway | What it does |
|---|---|---|
| **Pair a gateway** | B, the one being opened | Mints a *gateway pairing secret* and shows the `https` address to use with it. Lists the gateways that may open this one, each with its *remote start setting* and **Unpair**. |
| **Remote gateways** | A, the one opening | Lists the remote gateways, each folded under its name. Open a gateway's fold for its settings and **Remove**. **Add a remote gateway** opens the form: a short name, the pasted secret, optional extra request headers, then B's address, with **Pair** last. A row says **Access revoked** or **Not reachable** when B's own check says so. |

`lucidos pair --gateway` on B mints the same secret from a terminal. The
Workspaces picker also has **Add a remote gateway**, with the same form.

These rules hold, whatever the network:

- **A owns the remote workspace frontend.** A serves its installed client; B
  supplies workspace data and sandboxed content. B cannot run privileged browser
  code on A. Missing or incompatible client contracts require an upgrade
  on the gateway or engine named by the error (ADR 0723).
- **`https` only, except on one machine.** A refuses a plain `http` address,
  because the credential would cross the network in clear text. Give B an
  `https` address first: Route B below, or an *external address*. A loopback
  address (`localhost`, `127.0.0.1` or `::1`) pairs over `http` too, and A
  takes B's own certificate there. Use it for two gateways on one machine,
  such as the app and a dev checkout.
- **B must be new enough.** A reads `gateway_protocol` from B's health and
  refuses a gateway too old to pair. Update B, then try again.
- **Agent routing requires protocol 5.** Agent operations enter through their
  own engine, which verifies the calling thread. Pairing remains one-way:
  A's outgoing access gives B no access to A or A's other pairings.
  A separate return access grant permits B's agents to read and start threads
  through A. It grants no device identity or gateway management.
  Revocation blocks dispatch, including requests waiting on a peer.

  An uncertain spawn is never retried automatically.
- **A must be new enough too.** An A older than the *gateway pairing secret*
  wants 8 digits, and refuses the secret before it contacts B. Update A first.
- **A prefers B's external address** when no extra headers are configured,
  and only when A reaches the same B there. With extra headers it always keeps
  the address you typed, and sends headers only to that origin.
- **Secrets never cross over.** A device *pairing secret* or code never pairs
  a gateway, and a *gateway pairing secret* never pairs a device.
- **A terminal's secret waits for B's owner.** When `lucidos pair --gateway`
  minted it and B has a paired device, A's form says B is waiting. Tap
  **Allow** under **Settings → Access** on a device paired with B, and A pairs.
  A secret minted in B's Settings never waits.

**Unpair** on B refuses A's very next request. **Remove** on A also asks B to
forget the pairing. When B does not answer, A says so, and you unpair on B.

After **Unpair** on B, A's row says **Access revoked** and offers **Re-pair**.
Mint a new secret under **Pair a gateway** on B and paste it into that row.
Re-pair keeps the short name, the address and the extra headers. Agent return
access comes back only when it was on and a device paired with A taps
**Re-pair**. Otherwise the switch stays off and the row says why.

A row whose gateway does not answer says **Not reachable**, with B's reason.
Neither state polls: the section asks each gateway when it opens and on refresh.

#### Remote start setting

Each incoming pairing on B has a *remote start setting*: may agents on A
start threads on B? **Pair a gateway** shows it as a select on the pairing's
row (ADR 0725).

| Setting | A routed spawn from A |
|---|---|
| **Starts allowed** | Starts and runs at once. |
| **Starts ask me** | Starts parked on a **Remote start** card in the new thread. No model call runs, not even a title, until B's owner answers. |
| **Starts refused** | Is refused before any thread exists. The refusal names this setting. |

Every pairing starts at **Starts refused**, existing ones included. Reads
through the pairing are not affected.

A workspace call that A relays to B (a spawn without a route) has no card to
park on. B lets it through only under **Starts allowed**, and refuses it under
the other two. The refusal under **Starts ask me** names the routed spawn,
`lucidos spawn-thread --to <workspace>@<remote>`.

The **Remote start** card offers **Allow once** and **Deny**, and under
Remember **Always allow** and **Always deny**. A Remember pick also changes the
pairing's setting. If that change cannot be saved, the pick is refused and the
card stays open.

Only a tap on a device paired with B answers the card. A message typed into
the thread does not. **Stop** closes the card, and the thread never runs. Deny
ends the thread unstarted. A later message in a refused thread fails with a
reason.

The sending agent's spawn result carries `remote_start`: `Started.`, or a
note that the thread waits for B's owner.

B's gateway can be newer than a workspace engine that still runs. An engine
from before this setting cannot show the card, so B refuses every start to it,
under any setting, and nothing starts. The refusal says to restart that
workspace and retry. Reads to that engine still work.

#### Agent return access

If A pairs to B, A's own agents may use A's outgoing pairings.
For B's agents, A's owner opens **Settings → Access → Remote gateways**.
There they open B's fold and enable **Agent return access**.
Its tooltip names B by its registered name.
The switch is off for every new and existing pairing. An agent, local token,
or device paired only with B cannot enable it.

Both gateways need protocol 5, and participating workspace engines need routed
operation support. The switch checks B's current identity and compatibility.
B must reach A at A's **External address**, or, when none is configured, the
address of the paired device enabling the switch. If that address is not
reachable from B, configure a reachable External address on A and enable the
switch again. Loopback is only suitable for gateways on the same machine.

Turning the switch off blocks further dispatch through A immediately, even
when B cannot be contacted to remove its discovery entry. Stale discovery
grants no access. Removing either side's pairing also cuts the route. The
grant permits scoped reads and starting new threads, with the authentic source
thread's spawn permission. It gives no device identity or gateway management.
Use `lucidos workspaces targets --via <gateway>` for discovery and the routed
read commands in `system-knowhow/lucidos-cli.md`; keep the source-engine
environment throughout.

#### Manage a remote gateway's users and devices

A gateway pairing opens B's workspaces, not B's users and devices. The
*manage grant* adds that (ADR 0656). Devices on A may then list and revoke
B's devices. They may add, rename and remove B's users and their emails, and
move a device to another user. Nothing else on B is reachable this way:
pairing codes, gateway pairings, the external address and updates stay on B.
The *admin grant* opens those (§ Manage a remote gateway as its own device
would).

- **Off by default**, for every pairing, old and new. B's user turns it on
  for one paired gateway. `lucidos pair --gateway --manage` mints a secret
  whose pairing starts with it on.
- **The grant belongs to the pairing, never to a user.** Every device on A
  gets it, as every device on A gets B's workspaces.
- **A calls** `/~/api/v1/remote-gateways/<name>/manage/<call>`, where
  `<call>` is the same path as on B's own `/~/api/v1/auth/`: `devices`,
  `devices/<id>`, `users`, `users/<id>`, `users/<id>/emails`,
  `users/<id>/emails/<email>`, `devices/<id>/user`.
- **Refusals carry a `reason_kind`.** `manage-not-granted` (403) means B's
  user has not turned the grant on. `remote-gateway-too-old` (409) means B
  predates the hop. `remote-refused` (502) means B no longer accepts A.
- **In the app.** Inside a remote workspace (`/<slug>@<name>/`), **Settings →
  Devices** and **Settings → Users** show B's lists first, under "On <name>".
  Below them come the devices that use this workspace and A's own users.
  A refused grant says how to turn it on, and a B too old for the hop hides
  the section. On B, the switch sits on each row under Settings → Access →
  Pair a gateway.

#### Control a remote gateway's workspaces

The *control grant* lets devices on A create, start, stop and restore B's
workspaces (ADR 0811). It is a separate switch from the manage grant: one
reaches B's people, the other B's disk and memory.

- **Off by default**, for every pairing, old and new. A re-pair starts it off
  again. B's user turns it on for one paired gateway, with the second switch on
  its row under Settings → Access → Pair a gateway.
  `lucidos pair --gateway --control` mints a secret whose pairing starts with
  it on.
- **In the app**, the *gateway switcher* above A's Workspaces list picks the
  gateway. **This gateway** keeps the usual list of local and remote rows.
  Picking B lists B's workspaces with Start and Stop, and the footer reads
  **+ New workspace on B** and **Restore onto B**. Opening a stopped row starts
  it first, then opens it over the relay. An action B refuses stays off, and
  the footer says why: the grant is off, B needs an update, or B is silent.
- **Nothing destructive is reachable.** Delete, Trash, rename, snapshot
  restore and adoption stay with B's own devices. Pairing, the external
  address, TLS, updates and settings do too, unless the pairing also carries
  the *admin grant*.
- **A calls** `/~/api/v1/remote-gateways/<name>/control/<call>`, where
  `<call>` is the same path as on B's own `/~/api/v1/control/`:
  `workspaces` (list and create), `workspaces/<id>/start`,
  `workspaces/<id>/stop`, `restore-uploads` and `restore-uploads/<id>`
  (the chunked *restore upload*), `workspaces/restore` (the commit) and
  `restore-status`. A listed workspace carries its id, name, health, last
  error and unread count, never B's directory or port.
- **B keeps unread restore results across gateway relaunches.** A dismissal
  names the result the picker showed. If an older picker cannot dismiss the
  result, update the picker and try again.
- **One request carries one restore chunk**, at most 32 MiB. A proxy in front
  of B that caps bodies lower answers 413, and the upload shrinks its chunks.
- **Refusals carry a `reason_kind`.** `control-not-granted` (403) means B's
  user has not turned the grant on. `remote-gateway-too-old` (409) means B
  predates the hop. `remote-refused` (502) means B no longer accepts A.
  `remote-unexpected-answer` (502) means B answered with something other than
  JSON or plain text, which A never passes to the browser. It keeps a 413 when
  the page was a proxy's body-size refusal, so the upload still shrinks.
- **B's log names each change** with the device on A and the gateway it came
  through, for example `control POST /workspaces by 'My iPhone' via 'Gateway A'`.

#### Manage a remote gateway as its own device would

The *admin grant* lets devices on A use B's Settings **Gateway** group as a
device paired with B would. It includes the manage and control grants.

- **Off by default**, for every pairing, old and new. Only a device paired
  with B turns it on, with the third switch on its row under Pair a gateway.
  While it is on, the other two switches show on and locked.
- **Pair a gateway ticks grants up front.** Three switches above **Create a
  gateway secret** choose the grants the new pairing starts with.
  `lucidos pair --gateway --admin` does the same.
- **A calls** `/~/api/v1/remote-gateways/<name>/admin/<call>`, where `<call>`
  is the path under B's own `/~/api/v1/`: `control/gateway/status`,
  `control/gateway/reload`, `control/gateway/check-updates`,
  `control/release-check`, `control/installs`, `control/engine-builds`,
  `control/engine-builds/switch-running`, `control/network-config`,
  `control/external-address`, `control/tls`, `control/tailnet-status`,
  `auth/devices`, `auth/users`, `auth/gateway-pairing-code`, `auth/gateways`
  (list) and `auth/gateways/<id>` (unpair).
- **No grant is reachable from A.** The grant switches stay off the hop, and a
  secret minted through it cannot start with any grant. Such a secret
  needs no Allow on B, and B's log names the device and gateway that minted it.
- **Refusals** are `admin-not-granted` (403), plus the manage hop's others.
- **Both gateways need protocol 7.** Settings does not yet open B's Access and
  Updates & Installs pages through the hop.

**A remote behind an access proxy.** Prefer a tailnet address when both
machines have one. Otherwise enter the remote's HTTPS address and add its
required header names and values under **Extra headers** before pairing.
For example, a Cloudflare Access service token uses `CF-Access-Client-Id`
and `CF-Access-Client-Secret`; obtain both from the proxy administrator.
Lucidos's transport is provider-independent. A sign-in page means the proxy
has not admitted the request: supply its headers or use the tailnet address.

In **Settings → Access → Remote gateways**, open the gateway's fold. There
**Edit** on the **Headers** row replaces the
whole header set without pairing again. Re-enter every value you want to keep;
emptying the set clears it. Values are write-only: the list shows names and
“set”. The packaged macOS app seals values with Keychain; other installs use
an owner-only key file. Keep the original sealing key with a backup. Anyone
who can read both key and ciphertext can decrypt file-backed values.

Gateway credentials rotate automatically every 30 days. **Rotate** on the
fold's **Credential** row starts one immediately or retries a pending attempt. Errors and unsupported
peers appear on the remote row; update an older remote to enable rotation.
The pairing stays usable while retries run hourly. A durable sealed candidate
allows recovery after restarts and lost responses.

B accepts the old key for
up to ten minutes, ending the overlap when the candidate is first used.
**Pair a gateway** also shows when each paired gateway last used its grant.
Unpair remains immediate revocation; device credentials are unchanged.

Paired gateways relay voice and thread terminal connections. Both gateways
must support WebSocket relay. If a connection check fails, the client explains
what to check. The workspace's Voice and Terminal settings still apply.

While a gateway holds a pairing, in either direction, its engines land a
change on `origin` first and follow `origin`'s `main` (*origin-first
landing*). Unpaired engines land local changes locally and push after.

For source checkouts sharing an origin, choose one *lander* in Settings →
Coding Agents. The *landing setting* hands off queued snapshots through
origin in either pairing direction.

When A pushes a change to `main` on `origin`, it nudges each remote gateway.
B's engines then check for the new `main` at once, instead of at the next
`follow_main_interval_minutes` tick (ADR 0589). With `follow_main_build=auto`,
B builds in the background. With `follow_main_build=on-switch`, it follows
without compiling and offers **Build new version** when engine source is newer.

Once built, **Switch** still waits for a tap. Each workspace chooses its own
mode. Local Apply still requests a build. With `defer_version_builds=while-coding`,
automatic builds, including Apply, wait for this workspace's ongoing and blocked
coding-agent threads. The default `never` keeps automatic builds immediate.

The Lucidos menu names holding threads and offers **Build now**; explicit
rebuilds start immediately. Running builds continue. Requests coalesce into
one build of current HEAD after the threads finish. Each workspace counts
only its own threads; another workspace sharing the checkout may still build
its shared binary. An older B that
lacks the nudge just follows on its usual interval.

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
on. The gateway keeps the name, so a rename shows in every workspace on it.

**One rule names a device everywhere.** It is the typed name, else the pairing label.
Else it is the browser and machine from its user-agent plus the start of its id, as
in "Chrome on Mac (109371a3)". Failing all three, it is `device-` plus the
first eight characters of its id. The Devices row, a message's Origin popover,
an actor chip and the agent's list of your devices all follow it.

The gateway holds every device's typed name and pairing label, and each
workspace reads them from it. A workspace keeps only its own device state:
push, presence, subscriptions and preferences. Revoking a device forgets its
name too.

A message's Origin shows the device's current name, so a rename reaches older
messages too. The device you read on is marked "(this device)". The whole id
never shows: it is unreadable, and at that length it wraps.

### The tailnet-status endpoint

`GET /~/api/v1/control/tailnet-status` puts the Tailscale row in a browser.
The gateway answers it, once for every workspace it serves, and it needs a
paired device's credential like the rest of the control API. It returns
three fields, each a string or null:

| Field | Meaning |
|---|---|
| `magic_dns_name` | The name Tailscale reports, no scheme. Null off a tailnet, and null with MagicDNS turned off |
| `serve_origin` | The `https://<name>[:port]` origin of a `tailscale serve` route to this gateway, published only once verified. The page appends the workspace's own path |
| `reason` | Why the machine has no tailnet address, in one sentence. Null when it has one |

Only the machine can ask Tailscale for the name, so a browser has no other way
to learn it. The plain-HTTP row prefers the name over the bare
`100.x` address: it resolves anywhere on the tailnet, and a person can retype
it on another device.

**`serve_origin` is verified end to end, never inferred from a
listener.** With two gateways (443 fronting 5252, 8443 fronting 5251), a live
443 can belong to the other one. So the gateway fetches `/~/api/v1/health` at
each candidate origin (ports 443, 8443 and 10000), with TLS validated. It
publishes the lowest port whose `instance_id` is its own.

The probe presents no credential: a serve route can proxy anywhere (ADR 0602).
No candidate is probed unless Tailscale can issue a certificate for the name
(`CertDomains`), and every probe is bounded. The answer is reused for five
seconds.

It is a separate route from `network-config` on purpose: the bind editor
fetches that one too, and must not pay for asking Tailscale and a round trip.
The packaged app's own `serve_url` (from `get_connect_info`) answers "is this
MACHINE serving", for the Expose row. This endpoint answers "what is the URL for
THIS workspace".

**Tailnet state comes from Tailscale itself** (ADR 0588). The machine asks
Tailscale's LocalAPI for its status, the way the Tailscale CLI does, and falls
back to `tailscale status --json`. It reads the first IPv4 in
`Self.TailscaleIPs`, and `Self.DNSName` while MagicDNS is on. No interface name
or address range is ever used. So it reports correctly for the App Store app,
the standalone app, Homebrew or a Linux `tailscaled`, even with no `PATH`. When
debugging:

- **Not on a tailnet** always comes with Tailscale's own reason: not installed,
  not running, signed out, turned off, waiting for admin approval, or an answer
  that could not be read. The page and `lucidos pair --qr` both show it.
- A machine with **MagicDNS disabled** has a tailnet address and no name. It is
  still on the tailnet, with no HTTPS name to serve.
- A **Headscale** tailnet keeps its own MagicDNS domain, and gets no `https://`
  candidate: Headscale issues no certificates, so `CertDomains` is empty.
- **Another VPN in `100.64.0.0/10`** (Appgate, Zscaler, a corporate CGNAT) never
  takes the tailnet's place, because Tailscale does not report its address. To
  pair over such a VPN, set the external address (§ Any other VPN).
- In **userspace-networking mode** Tailscale reports an address this host does
  not hold, so the Network access picker offers no tailnet bind hint.
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

One case gets a Lucidos sentence first: a control server that answers the
feature-enable call with `501 Not Implemented` or `404 Not Found`, such as
Headscale. The toast says HTTPS cannot be served there, then quotes the CLI
after `Tailscale said:`.

One line is filtered out: `Warning: client version "..." != tailscaled server
version "..."`. Tailscale prints it on stderr for *every* command when CLI and
daemon versions differ. That is the normal state of a Homebrew CLI beside the
Mac app's daemon. With output streamed, it would otherwise lead every error.
Still resolve it before debugging `serve` in earnest (see § Operational notes),
though it is not the error.

