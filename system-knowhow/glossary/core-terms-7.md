---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### User
A person, as Lucidos knows them: a name, one or more emails, and the *paired devices* they hold. One device belongs to one user. The *workspace gateway* keeps users beside its devices, in its *gateway event log*, so every *workspace* it serves sees the same users. **Settings → Users** names users, adds their emails, and removes a user. A device's row in **Settings → Devices** moves it to another user.

Minting a *pairing secret* asks who it is for, and the device that uses it joins that user. A device that reaches this gateway through a *gateway pairing* shows as its user on the other gateway: "Sam", or "Sam (A)" when another user shares the name.

**A user is identity without authority.** It decides attribution ("Sam changed this trigger"), routing (whose *gate* it is) and read state. It never allows or refuses an action: every paired device can read, answer and change anything (ADR 0623).

Events keep the device, or an *actor* email for outside input, never the user. So moving a device or an email also moves the credit for older actions. Removing a user revokes all its devices, which is how a team offboards someone.

On a gateway with users to tell apart, the transcript names a teammate's message: their name leads the timestamp, and the bubble aligns left like the agent's reply. A device no user holds reads "Unknown user". The message's origin popover gains a User row. Your own messages look as before.

Formerly *operator*, and in the UI *person* or *people* (ADR 0655). Not to be confused with the user role of a model message.

Users can also talk to each other inside a thread, without its agent: see *user mention*, *agent mention*, *agent pause* and *user-only message*.

A gateway with one user hides team controls; Settings → Users still lists
that user so their name and emails are editable.
See also: *paired device*, *actor*, *team workspace*, *driver*, *user-only message*.

### Team workspace
A *workspace* that more than one *user* pairs devices to, run as the team's factory. The workspace is the only principal: it holds the bot credentials and does the work, and users steer it. No user-level permission exists (ADR 0623).

Tickets and chat threads reach it through *triggers*, each fire a *root thread*. Policy on where work stops for a user lives in knowhow, as *gates*. Questions and outcomes go back to the source through workspace script triggers, each a short message with a link to the thread. The *factory app* shows the team its open root threads.
See also: *user*, *root thread*, *gate*, *factory app*.

### Gateway event log
The append-only file in the *workspace gateway*'s data directory that records every pairing, revoke and *user* change. The device list and the users are rebuilt from it, so it is their only source of truth. It needs no Postgres, so devices still pair and sign in while Postgres is down. The older `paired-devices.json` moves into it once, with every device under one user.
See also: *user*, *paired device*.

### Pairing code
The 8-digit one-time code that makes a device a *paired device*, for a person who has to type. It works once and expires in five minutes. **It works only from another device on the LAN or the tailnet.** On the machine Lucidos runs on, and through a tunnel, a proxy or `tailscale serve`, Lucidos refuses it. It says to scan the QR or open the link instead (ADR 0615). Each code is minted beside a *pairing secret*, and using either spends both.

Two places produce one to pass on: **Settings → Access → Pair a device** (press **Create a pairing code**), and `lucidos pair` on the machine Lucidos runs on. The desktop app mints a third silently for itself: it reads the local file that proves it is on the machine, and pairs its own window on launch.

Both also draw a **QR**: the *pairing secret* wrapped in a reachable address. On your own network, typing the digits does the same, so a failed scan is never a dead end.

A desktop browser that scans it opens the pairing screen and spends the code at once. A phone browser gets the install steps instead, since the installed app is a different device from the tab. The code rides into that install, so the app pairs itself on first open. Install promptly: the code still lasts five minutes. An app already installed, or whose code ran out, takes one by **Paste** or **Scan QR** on its own pairing screen.

Only a process on that machine can mint one, which stops a stranger pairing in. An already-paired device can mint one too, so you can add a tablet from the sofa. A browser needs a code even on the same machine: proving you are local means reading a file, and a browser cannot. The desktop window is a browser by that test, so its Rust side mints for it.
See also: *paired device*, `system-knowhow/lucidos-cli/subcommands-14.md` § `lucidos pair`, `system-knowhow/remote-access/settings-access.md` § Settings → Access.

### Pairing secret
The long one-time secret minted beside every *pairing code*: `ldp_` and 26 more characters. The QR and the pairing link carry it, and the desktop app pairs its own window with it. It is long enough that no flood of guesses can find it, so Lucidos accepts it from any address and never rate limits it. A *gateway pairing secret* is the same idea for a *gateway pairing*, with no code beside it (ADR 0615).
See also: *pairing code*, *public listener*, `system-knowhow/remote-access/settings-access.md` § Pair a device.

### Pairing request
A *pairing code*, *pairing secret* or *gateway pairing secret* that waits for a paired device's **Allow** before it redeems. It happens when a process on the machine minted it, such as `lucidos pair` or the desktop window, while a device is already paired. Every process can read the local file that proves it is on the machine, an agent's too. So that file alone no longer lets a device in.

The redeemer waits on its own screen. Every paired device gets a notification, and **Settings → Access** lists the request under *Waiting for your Allow*, with **Allow** and **Don't allow**. A code minted on a paired device never waits, and with nothing paired nothing waits. When no paired device works any more, `lucidos devices revoke --all` clears them, and the next code pairs at once.
See also: *pairing code*, `system-knowhow/remote-access/diagnose-first-never-assume.md` § Devices pair before Lucidos answers them.

### Public listener
A second port on this machine's loopback, the gateway's port plus 30 (**5282** packaged, **5281** dev). Point a tunnel or reverse proxy here, rather than at the main port. It serves everything the main port does, but every request on it counts as public, so a typed *pairing code* is never accepted there. `LUCIDOS_PUBLIC_PORT` moves it, and `0` turns it off (ADR 0615).
See also: *pairing secret*, *external address*, `system-knowhow/remote-access/diagnose-first-never-assume.md` § Public only behind a front door you own.

### Pairing address
The address a *pairing code*'s QR carries, so the new device lands on the gateway that minted the code. Lucidos picks the first that exists, in this order:
1. the *external address*;
2. the tailnet address, Tailscale's or a self-hosted Headscale one;
3. the address you are reading the page on, if it is not this machine;
4. the local network address.

With none, Lucidos shows the code alone.
See also: *pairing code*, *external address*, `system-knowhow/remote-access/settings-access.md` § Pair a device.

### Gateway pairing
A one-way grant from one *workspace gateway* to another: gateway B lets gateway A open its *workspaces*, for every device paired with A. B mints a *gateway pairing secret*, and A pastes it. Each side records the pairing in its own file, so auth state stays per gateway (ADR 0590).

The grant is one-way: general access both ways means pairing twice. Pairing alone gives B no return authority. A serves its own frontend for remote workspaces; B supplies data and sandboxed content, without A's browser authority (ADR 0723).

**Unpair** on B ends it on A's next request. **Remove** on A ends it on A and asks B to forget it too. Gateways pair over `https` only, except on loopback, where nothing crosses a network.
See also: *gateway pairing secret*, *remote gateway*, `system-knowhow/remote-access/settings-access-2.md` § Pair a gateway.

### Gateway pairing secret
The one-time secret that makes a *gateway pairing*: `lgp_` and 26 more characters, pasted rather than typed. It works once and expires in five minutes. It is long enough that no flood of guesses can find it, so it is accepted from any address and never rate limited (ADR 0615). It never pairs a device, and a device's *pairing secret* never pairs a gateway. Two places mint one: **Settings → Access → Pair a gateway**, and `lucidos pair --gateway`.
See also: *gateway pairing*, *pairing secret*, `system-knowhow/lucidos-cli/subcommands-14.md` § `lucidos pair`.

### Manage grant
A switch on one *gateway pairing*, on the gateway being opened: devices on the other gateway may also manage this gateway's *users* and *paired devices*. Off by default. It belongs to the pairing, never to a *user*, and reaches nothing else on this gateway (ADR 0656). `lucidos pair --gateway --manage` starts a pairing with it on.
See also: *gateway pairing*, `system-knowhow/remote-access/settings-access-2.md` § Manage a remote gateway's users and devices.

### Return access grant
An optional grant on an existing *gateway pairing*, controlled by the gateway that holds the outgoing pairing. If A pairs to B, B gets no access back to A or A's other pairings. Only A's owner may enable return access so B's agents can read and start threads through A. Preserve the originating gateway, workspace, thread and source authorization. Off by default, one grant per source gateway.

Under **Settings → Access → Remote gateways**, enable **Allow agents on B to read and start threads through this gateway**. B means its registered name. `lucidos workspaces targets --via <gateway>` discovers routes; `--via` and `--to` select the routing gateway and workspace while keeping the caller's source-engine environment.

It creates no reciprocal pairing, device identity or *manage grant*. Revoking it or unpairing cuts the next incoming operation. Agents hosted by A need no return grant to use A's outgoing pairings (ADR 0725).
See also: *gateway pairing*, *remote workspace*.

### Trusted pairing
A *gateway pairing* whose owner turned on A's per-pairing toggle, so A serves B's own app for B's workspaces instead of A's. Off by default, because pairing does not prove who owns B.

Under **Settings → Access → Remote gateways**, turn on **Use B's own app for its workspaces**. B means its registered name. B's app then runs with A's access on every device that opens B through A. Turn it on only for a gateway you own. It keeps its own storage, apart from A's. B's service worker never runs.

If B is too old to serve its own app, A shows its own app with a banner saying so. It grants no *return access grant* (ADR 0807).
See also: *gateway pairing*, *remote gateway*.

### Remote gateway
Another *workspace gateway* this one has a *gateway pairing* with, listed under **Settings → Access → Remote gateways**. You give it a short name, such as `cloud`, when you add it.
See also: *gateway pairing*, *remote workspace*, `system-knowhow/remote-access/settings-access-2.md` § Pair a gateway.

### Remote start setting
A setting on one incoming *gateway pairing*, on the gateway being opened: may agents on the other gateway start threads here? **Allow** starts the thread at once. **Ask** starts it parked on a **Remote start** card, and no model call runs until the owner taps an answer. **Deny** refuses the spawn before a thread exists. Every pairing starts at Deny, existing ones included. Reads are not affected (ADR 0725).

Set it under **Settings → Access → Pair a gateway**, or with **Always allow** or **Always deny** on the card.
See also: *gateway pairing*, *return access grant*, `system-knowhow/remote-access/settings-access-2.md` § Remote start setting.

### Remote workspace
A *workspace* on a *remote gateway*, opened through this gateway at `/<slug>@<name>/`, such as `/dev@cloud/`. The Workspaces list shows it among the local ones in the *workspace order*, tagged `@<name>` and greyed while its gateway does not answer. Its gateway owns starting, stopping, renaming and deleting it, so this gateway offers none of those. A thread link names one as `thread:dev@cloud/<uuid>`. Inside a remote workspace, a link without `@` means a workspace on that same remote gateway, since that gateway's agent wrote it.
See also: *remote gateway*, *gateway pairing*.

### External address
The address other devices use to reach this gateway, when you run the route yourself: a reverse proxy, WireGuard, a company VPN, or a Headscale name with your own certificate. One per gateway: scheme, host and optional port, no path. It wins in the *pairing address*, and an empty one leaves the QR on the tailnet address. Saving it checks that it reaches this gateway. A failed check only warns; an answer from a different gateway refuses the save. Pairing still guards it: a device on it pairs like any other.

Set it at install (`--external-address`), with the `LUCIDOS_EXTERNAL_ADDRESS` environment variable (which locks the other three), with `lucidos external-address set`, or in **Settings → Access → External address**.
See also: *pairing address*, *paired device*, `system-knowhow/remote-access.md`.

### Parent thread
The direct ancestor of a *child thread*, resolved via the child's `parent_thread_id` column. A thread has at most one parent; a parent can have many children. Each child reports its outcome upward when it terminates, and the parent can send a *child follow-up* down to any child it spawned. A child paused by a user Stop sends a note instead; see *stopped child*.

### Part token
The custom property one *theme part* property compiles to, named `--part-<part>-<property>`, such as `--part-chat-text-text-shadow`. The engine emits it in the *theme*'s resolved map in canonical form, never the author's string. A shipped Lucidos stylesheet reads it under the part's selector, falling back to the element's unthemed paint. An app that loads the SDK stylesheet may read one by name. `--part-` is reserved: a theme's token maps cannot set one, and one in the *style overrides* must pass the same grammar.

### Thread pause
The "Paused" thread status: a *thread* whose turn the user's own *Switch to new version* interrupted, and which the engine has promised to resume. The whole thread is paused until then. Its indicator is the one that is not a dot: the standard pause glyph, in a neutral tone, never the red *failed* dot. The row's Info card says "Paused", and the transcript labels the turn "Paused by restart". The engine resumes the turn itself, usually within seconds, so no **Continue** button appears and nothing is asked of you. A paused thread does not count toward *attention*.

Every OTHER interruption is *failed*, with the red dot, an *attention* count, and Continue. That covers a crash, an engine shutdown the user did not ask for, and a switch whose resume the next boot cannot deliver. In that last case the boot replaces the pause with the error. So the pause glyph is a promise, never shown for a turn nothing is coming back for.

Paused is a *verdict* about the interrupted turn, not a resting state, so events that merely close the turn cannot walk it back to idle. A follow-up message clears it like any new work. Distinct from `waiting` (a *change* is in review) and `waiting_for_user_answer` (the agent is parked on a question).

**Not a stopped child.** A *child thread* paused by a user Stop waits on the user, not the engine. That makes it a *stopped child*, and it counts toward attention.

**A pending change does not change the verdict.** A thread interrupted mid-turn reads Paused (or *failed*) whether or not it carries a *change*. Nothing writes `waiting` for that now: the thread's change state carries the change, not its status. The change stays in the thread and the review list.

### Pinned thread
A *thread* the user pinned to keep at hand, in the Pinned section of the *thread drawer*. A pinned thread is never archived: pinning an archived thread brings it back, and archiving a pinned thread unpins it. So a settled pinned thread offers Archive beside Unpin. Only the user's own Archive unpins. Automatic archiving (a finished, unwatched trigger run) and an agent's archive leave a pinned thread alone.

Internally the pin is still called "saved": the `saved` section key, `is_saved`, and the `ThreadSaved` / `ThreadUnsaved` events.
See also: *Current section*, *thread drawer*.

<!--gloss-plugin-start-->
### Plugin
A bundle of installable workspace content shipped as one unit. Contains any of `apps/`, `knowhow/`, `triggers/`, `scripts/`, `auth-modules/`, `themes/`, `fonts/`, mirroring the top-level `data/` directories. Its `apps/` may hold *reusable widgets* the plugin owns. Defined by a *plugin manifest* at the root, beside an optional `media/` folder of *plugin media*.

Install merges the contents into the workspace's `data/`. Use a plugin when the pieces only make sense together (an app, its knowhow and its trigger); ship single files on their own.
See also: `system-knowhow/plugins.md`.
<!--gloss-plugin-end-->

### Plugin category
A topical tag on a *plugin* (e.g. `finance`, `health`, `developer-tools`) for browsing the *Plugins panel*'s catalog, as a per-category filter and chips on each *plugin detail page*. A **controlled vocabulary**: the author tags it in the *plugin manifest* (`categories = [...]`) from a fixed set. A value outside the set is dropped and flagged in the catalog scan's `errors`, never blocking install. Distinct from a plugin's *content* kinds (`apps`/`knowhow`/`triggers`/`scripts`/`auth-modules`), which the engine derives from the files. Allowed set and rationale: `system-knowhow/plugins.md`.

### Plugin manifest
The `manifest.toml` file at the root of a *plugin*. Declares `id`, `version`, `name` and `description`. Optional: topical `categories` (see *plugin category*), an *engine requirement*, install-time `setup` steps with an optional *setup revision*, and *plugin media* (`icon`, `screenshots`, `videos`). Schema in `system-knowhow/plugins.md`.

### Engine requirement
The Lucidos releases a *plugin* works on, declared in its *plugin manifest* as a semver requirement: `engine = ">=0.46.1"`. Set it to the first release with every platform feature the plugin uses. Omit it and any release will do, but Lucidos shows the gap: "No version requirement" on the *plugin detail page* until it is installed, and a note in the confirmation panel.

The engine enforces it at install and update only, never on an installed plugin. A release that misses it is refused before anything is written: "Theme Studio 0.1.0 needs Lucidos 0.46.1 or later". A malformed value is refused too. The *Plugins panel* still lists such a plugin, with Install or Update disabled and the reason beside it. No update notification is sent. See `system-knowhow/plugins/when-a-plugin-is-the-right-artifact.md` § "The `engine` requirement".

### Plugin local patch
The difference between a *plugin*'s content on disk and what it shipped at your installed version. The **Modified** badge counts it, as a diff rather than a yes or no. An update three-way merges it into the new version, so an edit you rely on survives. Where a merge is impossible, your version lands under `data/artifacts/plugin-local-changes/` with a `.patch` beside it. **Derived, not stored**: the engine diffs the working tree against the install commit, which records what the plugin shipped. The *plugin detail page* offers to send the patch to the plugin's author in a thread; see `system-knowhow/plugins/local-modifications-the-modified-badge.md` § "Local modifications".

### Plugin media
A *plugin*'s presentation files in its `media/` folder (icon, screenshots, videos, `README.md`), named by the *plugin manifest*. Lucidos never merges them into the workspace's content, and keeps an installed plugin's media, so it outlives its *marketplace*.

### Plugin detail page
The page a *Plugins panel* row opens in the *content pane*. It shows every screenshot and video plus the full description from `media/README.md`, and the chips the row leaves out. It carries **Uninstall**, and **Send changes to author** for a modified plugin. No plugin code runs there.

### Plugin widget
A *reusable widget* a *plugin* ships in its `apps/`, owned by the plugin rather than a thread: its manifest names `origin_plugin_id`. Any thread may embed or pin it. It offers no "Stop reusing", uninstall removes it, and deleting a thread never touches it. See `system-knowhow/plugins/shipping-widgets.md` § Shipping widgets.

