---
name: Remote Access & HTTPS, Settings → Access
description: Part of Remote Access & HTTPS. Its index lists the sections it covers.
---

One part of `system-knowhow/remote-access`, which lists every part.

## Settings → Access

The page that drives all of this: point the user here before hand-running
commands. It was called **Mobile Access** until the **Network access** bind
setting moved onto its bottom from Settings → System.

**It opens with Pair a device**, because pairing is what most readers came for
and the code already carries the best address. **Other addresses** folds under
it. Below those, the page answers two independent questions, numbered in setup
order, because the first must be true before the second buys anything:

| Section | Question | Where the answer comes from |
|---|---|---|
| 1. The machine running Lucidos | Is the engine's machine on a tailnet? | `detected_tailscale_ip` from `GET /api/v1/network-config` in any browser; the fuller `get_connect_info` probe on the packaged desktop app |
| 2. This device | Has the device reading the page joined that tailnet? | The address this device was served on |

**Section 1 carries an HTTPS row** when the page is served by the gateway
itself. It reads `GET /~/api/v1/control/tls` and says which certificate serves,
until when, or why there is none (§ Automatic: the gateway's own certificate).

**Pair a device is the action they lead to**, not a third question. It sits at
the top of the page; see its own section below. **External
address**, at the bottom of the page, is for a route other than Tailscale
(§ Any other VPN).

Every section renders **everywhere**, phone browsers and the installed PWA
included, Other addresses among them. Platform changes how much each can say,
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
| The machine's own MagicDNS name, or any `*.ts.net` name | "Tailscale is connected on this device" |
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
its tailnet. The machine's own name is matched exactly, so a Headscale domain
counts too. A request to the machine's tailnet address arrived over that
tailnet, and Tailscale itself reported that address. A loopback request never
left the machine.

A bare `100.64/10` host that does **not** match the reported address is not
proof. That range is real CGNAT space an ISP can hand to a physical interface.
An unproven host keeps the install offer, the harmless way to be wrong.

**The page never calls a device a phone unless it is one.** Desktop browsers
read it as often as handsets, and have no home screen. So the "add to home
screen" wording sits behind an iOS/Android check, and the neutral phrasing is
"install Lucidos".

**Other addresses** is a fold under Pair a device, closed until tapped. A
search hit on it lands open. It lists the addresses that reach **this
workspace**, for a route other than the QR's or a bookmark on a device that is
already paired. A new device pairs first, so it never needs one of these:

- **Local network**: only when the gateway serves a LAN address. Plain HTTP,
  so no PWA install and no push. Under a loopback bind (the packaged default)
  the row is absent; the **Network access** section further down says why.
  Packaged desktop app only, since detecting a LAN address needs the bridge.
- **Tailscale**: the tailnet address over plain HTTP until `serve` is verified,
  and `https://<name>.ts.net/<slug>/` once it is. Rendered **everywhere**,
  including a phone browser: see § The tailnet-status endpoint.
- **External address**: the user's own route, when one is set (§ Any other VPN).

There is no localhost row: that address is no use to another device.

**Every row carries the `/<slug>/` prefix**, because that addresses a workspace
(ADR 0014). A bare origin 307-redirects to the sole workspace or the picker,
the wrong address on an install with several workspaces.

Both plain-HTTP rows obey the network bind: being on a tailnet does not mean the
gateway **listens** on the tailnet address. Under the packaged loopback default
neither prints, because both URLs would be dead. A bind pinned to the tailnet
address shows the Tailscale row and no LAN row. `serve` needs no
wider bind, so the **HTTPS row is never bind-gated** and survives the packaged
default.

The bind that counts belongs to whichever process served the page. Behind the
gateway that is `gateway_bind`. On a direct engine port the origin is the
engine, which follows the gateway only while `[engine] inherit` is on.

### Pair a device

It opens the page, since pairing is what most readers came to do. It
mints a *pairing code* and draws it as a QR, so a phone scans rather than
typing eight digits. The reader is already paired, which makes the offer safe:
a paired device holds full authority and may enrol another.

**The section opens on one button, Create a pairing code.** No QR, digits or
address show until the user presses it. So when you walk someone through
pairing, tell them to press **Create a pairing code** first. Never promise a QR
on arrival.

**A live code shows as three cards**, one per way to use it: scan the QR (the
big card), type the digits, or open the address. The cards are alternatives, so
each after the first says "Or". The digits are always there, large enough to
read across a desk. On your own network or tailnet, typing them works when a
scan fails. Through a tunnel, open the address instead.

The two fallbacks
carry a Copy button, hidden where the browser has no clipboard (a plain-HTTP
LAN address is not a secure context). The address card is text, never a link:
following it here would spend a single-use code on a device already paired.

**The QR encodes `<reachable-origin>/~/#pair=<secret>`.** It carries the long
*pairing secret*, never the digits, so it works through a tunnel or proxy too.
The first head script captures and strips the fragment before resource requests or redirects.
The pairing screen redeems it through a POST body. Legacy query links remain accepted. On a phone the scan lands somewhere else: see § A phone
installs before it pairs.

**The address is the hard part.** The machine minting a code usually reads this
page over loopback, and a QR aimed at `127.0.0.1` helps nobody. So the section
takes the first *pairing address* that exists (ADR 0550):

1. The gateway's external address, when one is set (§ Any other VPN).
2. The tailnet origin, as the Tailscale row derives it: the verified `serve`
   origin, else the MagicDNS name, else the tailnet address, each only while
   something listens on it.
3. The address this page is on, when it is not loopback.
4. The LAN address, on the packaged desktop app.

With none of those it mints the code and says there is no QR. With no external
address set, which is the default, a machine on a tailnet pairs over it.

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
client-created manifest's fragment `start_url`, and the app spends it on sight with nothing typed. To
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

Private path settings are available from the Lucidos shell on a paired device.
The workspace owner can also manage standing denies in **Settings → Permissions**.

Access adds a device. **Settings → Devices** lists every device and holds
**Revoke**. One row per device carries both actions:

| Action | Reach | What it does |
|---|---|---|
| **Revoke** | the whole machine | Stops this device reaching Lucidos, on every workspace. |
| **Remove** | this workspace | Forgets its push subscription and its preferences here, and leaves it paired. |

Device preferences include **Agent brand colours** in **Settings → Appearance & Behavior**.
Removing a device forgets that choice here along with its other preferences.

Both halves key on the id the gateway minted at pairing, so each device has one
row. (There used to be separate **Paired devices** and **Devices** lists.)

An unused device folds into a collapsed **Unused devices** group at the end.
Unused means not this device, no push and not paired. Expand the group to
**Remove** one.

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

### Users: whose device is which

**Settings → Users**, beside Devices, names the people who pair devices to this machine.
Each is a *user* in the glossary. Users belong to the gateway, so every
workspace it serves sees the same users. A personal machine has one user,
who holds every device. Settings lists that user too, so you can rename them
or add an email without adding a second user.

Each user row has **Add email**, **Rename** and **Remove**. An email is how
Lucidos recognises that user across gateways, issue trackers and chat tools.
**Remove** revokes every device of that user, then forgets them.

Once a second user exists:

- Each paired device row gets a user picker.
- **Settings → Access → Pair a device** asks who the code is for. The device
  that uses it joins that user. Without a choice, it joins the first user.

A user grants nothing. Every paired device can still do everything, whoever
it belongs to (ADR 0623).

A device that reaches this machine through a paired gateway shows as its
user on the other gateway, "Sam". When another user shares that name, the
gateway joins it, "Sam (laptop)". When that user has an email that a user
here also has, it is the same user and shows the local name: one human who
pairs their own laptop and server counts once. So give your user the same email on both
gateways. Ending the gateway pairing forgets the users it relayed.

The gateway keeps devices and users in its *gateway event log*, in its data
directory, so they work while Postgres is down. The same list is on the
gateway API: `GET` and `POST /~/api/v1/auth/users`, `PUT` and `DELETE
/~/api/v1/auth/users/{id}`, `POST /~/api/v1/auth/users/{id}/emails`,
`DELETE /~/api/v1/auth/users/{id}/emails/{email}`, and `PUT
/~/api/v1/auth/devices/{id}/user` with `{"user_id": "..."}`. A
pairing code names its user with `POST
/~/api/v1/auth/pairing-code?user=<id>`.

Gateways and engines may update separately. During the user rename, HTTP
adapters keep older counterparts' identity lists and pairing selections intact.
Stored history still migrates to the new names (ADR 0655).

A device that reaches this machine through a *gateway pairing* shows as its
user on the other gateway, such as "Sam". It shows as "Sam (laptop)" when the name is shared.

