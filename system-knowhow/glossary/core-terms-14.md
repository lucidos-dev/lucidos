---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Ingress probe
Lucidos knocking on its own public webhook address from outside, every 15 minutes, to check a sender could still reach it. It sends one unsigned POST to a real *webhook* and expects to be turned away. A refusal is the good answer: it proves the whole path is alive, from the public relay through the *hook socket* to the verifier.

**It leaves the machine, and probes every address separately.** A loopback check would pass while the outside world got nothing. The public name usually has several addresses and a sender reaches one, so each is probed and judged per address family. An IPv4 outage is an outage even while IPv6 answers, which is the failure this catches.

It runs only with something to protect: at least one enabled webhook, and a funnel serving the hook port. Two failed rounds in a row declare an outage, and one good round ends it. A round that reached no address declares nothing and changes nothing, since it measured nothing. You see a bar across the app and a line on every enabled row in **Settings > Webhooks**. The bar's **Discuss** button hands the agent the whole diagnosis, address by address.

Lucidos only reports. It emits `WebhookIngressDegraded` and `WebhookIngressRecovered` once each per outage, not per round, and a *trigger* decides what to do. It never re-arms the funnel: that is your tailnet, and the fix depends on who should be delivering.

**A round your computer slept through proves nothing.** It cannot declare an outage, and after a wake the count starts over, so a sleeping laptop never reads as a dead funnel. Senders like GitHub do not resend a delivery that failed, so after a sleep of 30 minutes or more Lucidos emits `WebhookDeliveriesSleptThrough` once. It sends no notification unless a *trigger* asks for one.
See also: *webhook*, *refusal check*, *trigger*, `system-knowhow/remote-access.md`.

### Refusal check
The other half of the *ingress probe*: what a *webhook* does with deliveries that DO arrive. Also every 15 minutes, but it sends nothing. It reads each webhook's own record of what it accepted and turned away.

**The probe cannot see this fault, by design.** It expects to be turned away, so a webhook that refuses every real delivery passes it perfectly. A hook switched off by hand once threw away eighteen days of deliveries with every check green.

A webhook is reported after at least three refusals spanning at least half an hour. Both are needed: three refusals in four seconds is one bad payload. **A switched-off webhook is reported after one**, since nothing was read and the delivery is certainly gone.

The two cases always use different words. A switched-off webhook verified nothing, so its signature and secret are fine and the fix is the Enable button. Calling it a signature problem would send you looking where there is nothing to find.

You see a bar across the app and a line on the webhook's row in **Settings > Webhooks**. The bar's **Discuss** button hands the agent the declaration: which webhook, which case, how many deliveries and for how long. Use it for the verification case, to check the secret still matches the sender's. It clears when a delivery verifies, the webhook is switched back on or deleted, or nothing arrives for a fortnight. It emits `WebhookDeliveriesRefused` and `WebhookDeliveriesRecovered` for a *trigger* to act on, and fixes nothing itself.
See also: *webhook*, *ingress probe*, *trigger*.

### Widget
A small interactive answer that lives in a thread: an *app* whose *app manifest* says `"kind": "widget"`, made by `create_app` with `kind: "widget"`. The agent's reply embeds it, so it shows in the thread as an *inline widget*. It draws into a *widget box* with no scroll area inside, and its name says what it does ("Flight picker"). It runs in the same frame as an app and reaches exactly what an app reaches. It is never in the apps list, so the list stays the user's persistent interfaces.

The thread filter lists each widget a coding-agent thread worked in under its own Widgets heading. A deleted widget stays there, marked deleted, because `AppDeleted` records its kind (ADR 0404).

Its manifest records its one owner: the thread that made it (`origin_thread_id`), or the *plugin* that ships it (`origin_plugin_id`). *Delete (a thread)* removes that thread's own widgets. Uninstall removes a plugin's.

"Make app" builds a separate, full-size app from it and leaves the widget as it is. Not an OS home-screen widget, which is a presence surface outside Lucidos.
A widget can take *widget params*, so one widget shows in many places with different content: a sound player with a different clip each time. A *widget embed* draws it wherever a picture draws.
See also: *reusable widget*, *built-in widget*, *widget instance*, *inline widget*, `system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets, ADRs 0402, 0415, 0466 and 0621.

### Reusable widget
A *widget* whose manifest says `"reusable": true`, so any thread may embed it. The apps panel lists them in a Widgets group, and the agent sees them in its prompt. Its origin thread stays recorded: "Stop reusing" clears the flag, and the widget then belongs to that thread again. So it is refused once that thread is deleted, and while a *dashboard* holds the widget. Deleting the origin thread never removes a reusable widget. A *built-in widget* is a reusable widget that ships with Lucidos, and a *plugin widget* one a plugin ships, and neither offers "Stop reusing".

### Inline widget
A *widget* drawn in a turn, also called its card: its bare *widget box*, centred in the turn, drawn as its dashboard tile is. A right-click, a long press on a phone, or the menu key on the focused card opens its menu. The menu starts with a Size row, then Open in Canvas, Add to dashboard and the rest, the same on every device.

A reply draws one from each *widget embed* it holds, and an old `WidgetShown` still draws one at its turn. It starts at the widget's default size. The user's size pick sticks to that *widget instance* in the thread, on every device and after a reload (`InlineWidgetSized`). The agent sets one with `widgets(action="size")` only when you name a size (ADRs 0466, 0621).

### Widget box
The fixed box a *widget* draws into wherever it has a *widget size*: its *inline widget* in a thread and its tile on a *dashboard*. Both draw the same box at the same size, so what the thread shows is what the dashboard shows. The host sets the box, content never sizes it, and a host narrower than an XL box scrolls the box sideways. The widget fits by showing less, never by shrinking its text, and never scrolls inside. Content past the box is clipped under a fade, and Open in Canvas shows the widget full size. A question option and the Canvas draw a widget with no box (ADR 0621).

### Widget fit check
A check that renders a *widget* at every *widget size* it declares and says what each *widget box* cuts off. An open Lucidos window draws each box off screen, on the desk tile and on a 360 px phone column. It uses the largest UI scale any device uses. The engine writes the answer as a `[WIDGET FIT]` note.

The note rides the tool result when the agent creates a widget or writes its own files. `widgets(action="fit")`, `lucidos widgets fit` and `POST /api/v1/widgets/fit` run it on demand. It never refuses or changes a write. With no visible window, no answer, or a box it could not measure, it says `not checked`, never a pass.

See also: *widget box*, `system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § The widget box, rule 8.

### Widget shape
A class the app stylesheet serves for a *widget*'s main shape: `.ring` for a gauge, `.bar-run` for a short series, and `.choice-row` for the tap targets of a pick. Each draws from the theme tokens and follows the motion setting. A widget leads with one instead of a table (`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets, `system-knowhow/js-sdk/setup-2.md` § Widget shapes).

### Widget params
The JSON object a *widget* receives for one place it is shown, read with `lucidos.params()`. It arrives in the frame URL as `?params=`. Its *app manifest* declares the names it takes. Params are visible in the URL, so they never carry a credential (ADR 0415).

### Widget embed
`![label](app:<id>?params={...})` in markdown, which draws the *widget* live wherever a picture draws: an option's description or preview, a card's `message`, a reply. It is the one way a widget shows in a thread, and its alt text is the label. In a reply it draws an *inline widget*; in an option, the widget alone inside the option's box. An embed records no event. A bad embed in an option refuses the card. A bad one in a reply reads "This widget is not available" and gives the reason (ADRs 0415, 0466).

### Widget instance
One *widget* with one set of *widget params*. A card size and a *dashboard* tile key on it, so two instances of one widget can each have their own. Its label is not part of the key. The same names and values in a different order are one instance (ADR 0415).

### Widget size
One of four sizes a *widget*'s *widget box* takes, in a thread card and a *dashboard* tile alike. An *app shortcut* has its own, `quarter`, which no widget takes. The four are `small` (one cell, square), `medium` (two cells wide, one high), `large` (two by two) and `xl` (four wide, two high).

Cells are square and scale with the UI scale. A cell keeps its size at every dashboard width: the dashboard has as many columns as fit, centred, and only one too narrow for two draws them smaller. An `xl` tile or card can be wider than its host. The host then clamps it to the room and scrolls the box sideways at its true size. A widget that declares no sizes takes small, medium and large and starts medium, because `xl` is opt-in. One that declares `"sizes": ["medium", "small"]` in its *app manifest* takes only those, the first as its default.

The box sets the size and the widget draws for it, with no scroll inside. It reads its size from `data-widget-size` on `<html>` or `lucidos.widget.size()`. Content past the box is clipped under a fade, and the menu opens the widget full size in Canvas. A size the widget no longer declares falls back to the nearest smaller one it does (ADRs 0432, 0621).

### Built-in widget
A *reusable widget* that ships inside Lucidos, under an id starting `lucidos-`, read-only, updated with each release. Two ship: the sound player, `lucidos-sound-player`, and the video player, `lucidos-video-player`. No workspace app can take a `lucidos-` id. "Make app" gives an editable copy (ADR 0415).

### Workspace
A user's complete Lucidos instance: one PostgreSQL database inside the shared
Lucidos Postgres cluster, plus a `data/` directory (artifacts, apps, knowhow,
triggers, intents, config, auth-modules, scripts, themes). Multiple workspaces run
concurrently, each its own isolated engine and database. A single *workspace
gateway* (dev term) fronts them and addresses each by its *workspace address*,
the path prefix `/<slug>/`.

The workspace picker is at `/~/`, or just `/` when there is more than one. There
you switch, create, rename and delete workspaces, and toggle each one's
*auto-start*. You can also **restore one from a backup**: drop in an encrypted
`.age` backup file and its *backup key*, or an older `.enc` file and its key.
The name comes from the backup, and you must change it if its address is taken. Every workspace you have launched stays
**listed** after it stops, and opening a stopped one starts it.

You can also **switch without the picker**. The Lucidos menu's Workspaces row
(tap the Lucidos mark in the header) unfolds the same list, marking the current
one. Each row shows whether that workspace is running. Unread counts sit in the
menu's notifications rows above the list, not on these rows.

The installed desktop app gives each workspace its own window, and brings it
forward if already open. A browser tab or installed web app switches in place;
in a browser, cmd-click or middle-click opens a tab. A right-click always offers
the other one.

Only switching lives in the menu. Creating, renaming, deleting, restoring and
starting or stopping go through the picker, linked from the list's last row. An
agent does them through the *workspace broker*. It stops, starts, renames and
sets autostart on a *machine-made workspace* with no card, and asks you for
every other act. On
first run the picker offers both ways in side by side: name your first workspace
(suggesting "personal" or "work"), or restore one from a backup. Nothing is
auto-created.

### Workspace order
The order of the one Workspaces list, local and *remote workspaces* mixed. You
set it in the workspace picker: drag a row by its grip, hold it on a touch
device, or pick Move up or Move down in its ⋯ menu. The Lucidos menu's list
follows the same order but cannot change it.

The gateway stores the order, so every device paired to it sees the same list.
A workspace you never placed goes at the end: local ones first, in the order
they were created, then remote ones by gateway. Two gateways keep separate
orders. Renaming a remote gateway sends its workspaces to the end until you
place them again.

### Workspace origin
The provenance recorded when a workspace is registered: `user`, `agent`,
`eval`, `e2e`, `demo` or `unknown`. Older registrations are `unknown` and
remain protected. Only a paired device may lower an existing workspace's
protection. A local caller may raise it to `user`. The owner's Run all on a
*workspace broker* classify batch may move an `unknown` one. A familiar name or
folder prefix does not establish an origin.

### Machine-made workspace
A workspace whose *workspace origin* is `agent`, `demo`, `eval` or `e2e`: a
harness, a rig or an agent made it, not a person. It is the agent's: through
the *workspace broker*, an agent stops, starts, renames and sets autostart on
one with no card. An agent may delete or restore only these, with a card. It
may restore a snapshot only into one, with no card. A `user` or `unknown` workspace stays the
owner's, in the picker.

### Workspace broker
How an agent controls this machine's workspaces without the local token:
`lucidos workspaces create`, `stop`, `start`, `autostart`, `rename`, `delete`,
`restore` and `classify`. The agent asks its own engine, which asks the
gateway. Stop, start, autostart and rename on a *machine-made workspace* act
with no card. Every other call needs the owner's *permission* for that verb,
asked on a *permission request* card.

Delete and restore take a machine-made
workspace only. There is no restart: stop, then start. What it creates has origin
`agent` and *auto-start* off.

### Ephemeral workspace
A workspace explicitly registered as disposable by its creating harness. Its
owner token authorizes cleanup only for that registration while it remains
eligible for harness cleanup. An expiry date suggests it for review; nothing is
deleted on a timer. User and unknown workspaces require a paired device.

### Trash
Recoverable workspace and worktree state retained by deletion. The workspace
picker lists deleted workspaces in Trash and asks for named confirmation before
moving them there. Restore checks for collisions and recovers the saved state
without starting an engine. Entries stay until a restore or until a paired
device empties Trash.

### Workspace address
The path a *workspace* is served at (`/personal/`), also its folder and database name. It derives from the name at creation (lower-cased, anything not a letter or digit turned into `-`). Then it is **fixed forever**: a rename changes the label, never the address. The picker shows the address only when it would surprise you: after a rename, or when two workspaces share a label.

**No two workspaces can share an address, and no two can share a name either.** Creating or renaming to a taken name is refused, naming the holder. Names match ignoring case and surrounding spaces, since "Work" and "work" look the same in a list. A name a running restore is about to use counts as taken, and you can wait for it.

The address alone may be taken, by a workspace since renamed. A create then gets the next free address, `/personal-2/`, and the picker says so first. A restore is refused instead, since a suffixed address would quietly make a second copy. Workspaces that shared a name before this rule keep working, and the picker shows their addresses.

### Workspace font
A font installed into the *workspace*, by you, the agent or a *plugin*, rather than shipped in the *font catalog*. It lives in `data/fonts/<slug>/`: a `font.json` naming its label, group and font files, beside the files. Its id is `ws-<slug>`.

The group decides where it fits: a `sans` or `serif` font can be the UI font, and a `mono` font can be the UI font or the code font. The engine serves it from the workspace, never from the internet, and lists it after the catalog fonts in `GET /api/v1/fonts`. The `font-family` *preference* and a *theme* name it like any catalog font. See `system-knowhow/workspace-fonts.md`.

### Workspace audit
A read-only sweep of what your *workspace* holds against how Lucidos expects it to look today: *app*s, *trigger*s, *knowhow*, intents, scripts and *artifact*s, plus the *workspace prompt footprint*. It writes a report and changes nothing. Ask for it by name, or take the button on the *release notice* that offers it. Drift is mostly silent: a trigger on a renamed event stops firing, and an app that lost a browser capability quietly forgets your settings.

It is split into *audit sections*, and your words pick how many run. An unqualified "audit my workspace" is the **full pass**: every section, every time, compared with the last report but never inheriting its coverage. Naming a section ("run the prompt footprint audit") runs that section only. Naming one break ("migrate my apps off `localStorage`") is a **targeted run**: one check and its fix guidance over one surface, straight through to the fixes. Anything short of a full pass reports only what it examined, so it never reads as a clean bill of health. Otherwise fixes happen only if you ask: the sweep proposes, and a *coding-agent thread* per app does the work.

Driven by `system-knowhow/workspace-audit`. Siblings: *workspace learning* asks whether the conventions match you, and the *setup interview* asks whether the workspace matches you.

### Audit section
One part of the *workspace audit* that runs on its own, such as triggers, apps or the *workspace prompt footprint*. A full pass runs every section. Naming one runs just that section. Each lives in its own file under `system-knowhow/workspace-audit/`, which the audit's root names.

### Workspace prompt footprint
What your *workspace*'s own content adds to every *Lucidos Agent* turn: the *app*s, *reusable widget*s, *knowhow*, intents, accounts, profile and *MCP server*s the agent is told about. Distinct from the *system prompt footprint*.

It is measured per section against two ceilings, one per section and one for the total. Both are *preference*s you can change, beside a third: how many days without use make an app or reusable widget count as unused. A knowhow doc can be read through the shell, which leaves no trace, so one is only ever "not loaded by name". Lucidos never cuts a section to fit.

The MCP tool schemas are listed per server, with the threads that enabled and called each one, but kept out of the total: only a thread that enables a server carries them. The *workspace audit* flags what is over, and Settings → System → Workspace Prompt Footprint shows it. `lucidos workspace-prompt-footprint show` prints it.

### System prompt footprint
What Lucidos itself sends on every *Lucidos Agent* turn: its rules, tool schemas and the System Knowhow list. Fixed by the release, not by your *workspace*, and not something you can trim. Settings → System → Workspace Prompt Footprint shows it beside the *workspace prompt footprint* for scale.

