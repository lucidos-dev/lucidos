---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

<!-- domain-event: JobListingFound: Example domain event emitted by a job-listing integration. -->
<!-- domain-event: PanasonicHeatpumpAdjusted: Example domain event emitted by a heat-pump integration. -->
<!-- domain-event: MorningRoutineCompleted: Example domain event emitted by a morning-routine trigger. -->

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Secret mention
A `CRED_*` or `OAUTH_*` variable name, or a credential's custom alias, written
as a whole word in the text a subprocess runs. That text is the `run_bash`
command or `run_python` code, plus each workspace `.py` / `.sh` file it names.
A scheduled script's text is its own file. **A subprocess gets a credential
only through a secret mention.**

A name built at run time is declared with a header comment,
`# lucidos-credentials: CRED_A CRED_B`. A bare `CRED_` names nothing.
`lucidos credentials list` shows the names to use.

### Secret reference
A *credential* field that points at a secret in 1Password instead of holding
it: `op://vault/item/field`, or `op://vault/item/section/field`. Any secret
field can hold one: an API key, a token, a password, an OAuth client secret.
Turn on **From 1Password** beside the field, then paste the reference. Copy it
from the item's field menu in 1Password.

Lucidos reads the value through the 1Password command line tool (`op`) with a
read-only service account token. Set both up under **Settings → Accounts →
1Password**, and use Test there to check them. The token reaches only `op`:
no script, request or reveal ever gets it.

Lucidos fetches the value only when something uses that credential, and keeps
it in memory for 10 minutes. It is never written to disk. Saving checks the
reference once, so a typo is refused. Revealing the field shows the reference,
not the value. If 1Password is unreachable, the last value keeps working for
up to a day. If the item is gone or access is denied, the credential fails at
once and you get a notice.

### Backup recipient
The public half of a workspace's *backup key* pair, an age public key starting
`age1`. Every new backup is sealed to it. Lucidos stores only this half, so it
can make backups but never open them. Set it up under **Settings → System →
Backup**, or paste an age public key you already own. That page shows it once
it is set, so another workspace can use the same backup key.

### Backup key
The private key that opens a workspace's backups, starting `AGE-SECRET-KEY-1`.
Lucidos shows it once at setup and never stores it. Backups stay off until you
paste it back, which proves you saved it. Keep it in a password manager: a lost
backup key cannot be recovered, and neither can the backups.

Settings offers **Verify** (paste it to check it still matches, suggested
yearly) and **Rotate** (a new pair; old backups still need the old key). A
backup is an age file (`.age`), so the `age` command line tool opens it too.
Backups made before age (`.enc`) need the older base64 key, which Settings
showed before the switch.

### Cross-gateway link
A link to a *thread* in a *workspace* your OTHER Lucidos install serves. Most machines run one install, where no link is cross-gateway. A machine running the packaged app beside a dev checkout runs two, each with its own workspaces.

It opens in a tab on that install's own address, port and all. Pairing is per gateway (see *paired device*), so a browser that install has never met lands on its pairing screen. You reach it from the machine, or wherever you reach Lucidos on its own port. A forwarded address (`tailscale serve`, an ssh tunnel) has only one install on the far end. There the link says where the workspace lives instead of opening a tab that cannot load. A workspace neither install knows still says it is not available.

### Dashboard
A grid of live *widget* tiles and *app shortcuts* in square cells. **There are two dashboards, `phone` and `desktop`, and each has its own tiles.** A screen in the phone's swipe layout shows the phone dashboard, and one in the split layout shows the desktop dashboard, a tablet included. A widget tile is a *widget instance* at one of its *widget sizes*, live like any widget frame. An app shortcut is a quarter-size tile that opens its app, and both kinds sync across devices. Before ADR 0496 this was the "Home board": one tile set, the widgets pinned to Home.

**A dashboard settles before it shows.** Its grid stays hidden behind a skeleton until every widget frame on screen has loaded, or three seconds have passed. Then it fades in as one, once per mount.

**On a phone, the dashboard is a *dashboard panel* in the thread pane, over the open thread.** A button beside the Lucidos mark in the thread header opens it, on every thread, Home included. Home is a plain chat there, as on a desktop.

**On a desktop, the dashboard is a rail at the window's right edge**, the full height of the window, beside the header and the panes. It stays in view whatever the Conversation and the Canvas show. Drag its left edge to resize it: it is two cells wide at least, and wider shows more columns. Its button, the dashboard icon in the window's top-right corner, reads Show dashboard or Hide dashboard and stays put either way. Each device remembers its width and whether it shows (ADRs 0466, 0468).

**Add to dashboard** is the "+ Dashboard" button on a widget's card, or the item in its menu. It adds the widget at the card's size to the dashboard of the layout you tap it in, from any thread, Home included. A widget that belongs to its thread becomes reusable first, in the same action, so deleting the thread never empties the tile. Each dashboard also has its own Add, which lists the reusable widgets, then your apps. An app's row in the Apps panel has Add to dashboard too.

**Remove from dashboard** takes the tile off that dashboard only. That is how a phone shows fewer tiles than a desktop.

**Arrange it in edit mode:** Edit in the dashboard's header, or Edit dashboard in a tile's menu. A long press or a right-click on a tile opens that menu. Drag a tile to move it, tap its size pill to resize it, or remove it. A new widget tile starts at its widget's default size. An app shortcut has no size pill, since it has one size.

**The agent names a dashboard, or takes the default.** Its `widgets` tool reads one with `action="dashboard"`, and changes one with `add_to_dashboard`, `remove_from_dashboard` and `arrange`. It arranges only when you ask. With no dashboard named, it uses the *last used device*'s: an iPhone or an Android phone means `phone`, and anything else `desktop`, an iPad included. A turn with no device, such as a trigger run, must name one.

The CLI has `lucidos widgets dashboard`, `dashboard-add`, `dashboard-remove` and `arrange`. Over HTTP they are `GET /api/v1/dashboard` and `POST /api/v1/widgets/dashboard-add`, `dashboard-remove` and `arrange`, and an omitted dashboard means the calling device's.
See also: *dashboard panel*, *home thread*, *app shortcut*, *widget size*, *widget box*, *last used device*, ADRs 0432, 0455, 0466, 0496, 0532, 0538 and 0621.

### Dashboard panel
The phone *dashboard*, a panel in the *thread pane* over the open thread. A button on the right of the Lucidos mark in the thread header toggles it, with or without a thread open. It fills the thread pane below the header. The header, the Lucidos menu, the menu drawer and a *pane swipe* all keep working while it is open. It slides away with the thread pane and is still open when you swipe back.

Opening it adds a row to the thread history, so the thread header's back chevron closes it and forward reopens it. Closing it with its button is a Back step, so toggling it never adds more rows. A screen that leaves the phone layout drops those rows. Opening any thread closes it, the open one included, as from a notification. A reload reopens it if it was open over a thread. While open, the button wears the threads Filter button's on look.

**Opening it never changes the thread you are in.** A *widget* tile that asks which thread is focused gets the thread under the panel, so a thread-aware tile describes that thread. It keeps its tiles live while closed, so a reopen shows them as they were.

The desktop has no panel: its dashboard is the rail. See ADR 0538.

### App shortcut
An *app* on a *dashboard*: its icon and name, and a tap opens it in the Canvas. **It is a quarter tile**, half a small cell's edge on each side, so four fill one small cell. It has no border and no fill: the icon and name sit on the rail, as on a phone home screen. Edit mode outlines it. No frame mounts, so an app never runs inside its tile. It has one size and takes no *widget params*.

Add one from the dashboard's Add, an app row's Add to dashboard, `widgets(action="add_to_dashboard")`, `lucidos widgets dashboard-add --app-id`, or `POST /api/v1/widgets/dashboard-add`. The app's manifest decides whether an id is a shortcut or a widget tile. A deleted app's shortcut drops off (ADR 0532).

### Draft
What you typed into a *thread*'s composer and have not sent: text, images, or both. It lives with its thread and follows you to your other devices. A never-sent thread is all draft and shows in the **Drafts** group of the *Ongoing grouping*. A thread with history can hold one too, archived or not.

The `threads` tool's `drafts` action lists each draft with a preview, last edit time and owning thread, and returns one whole by `thread_id`. `lucidos threads drafts` is the same read. An agent sends your saved draft only when you tell it to, or approve its card: `submit_draft`, or `lucidos threads submit-draft`, sends it exactly as saved (ADR 0651). A draft has no link of its own, so the agent gives its thread's *thread link*. Distinct from a *held message*, which an agent wrote and Lucidos keeps back.

An agent inside a thread can hand you a ready message as a draft in a new thread: `lucidos threads create-draft` (ADR 0665). It never opens that thread, never sends, and never touches a draft you already have. The drafts read names the agent's thread as `drafted_by`.

### Draft revision
A counter on a *thread*'s saved *draft*, `snapshot.draft_revision` in a one-draft read. It goes up only when the draft changes: its text, images, mode, picks or *compose epoch*. Activity, a new title or a status change leave it alone. An agent names it, with the compose epoch, to send exactly the draft it read (`submit_draft`, ADR 0651). A changed draft refuses the send as stale.

### Disabled tool
One tool on an *MCP* server that the user switched off, so the *Lucidos Agent*
is never offered it. The rest of that server's tools keep working.

In a thread that enabled the server, every tool's definition rides on
**every** request. So a forty-tool server costs tokens each turn there even
when the agent needs two. Switching one off
removes its definition, and the per-request token total drops. A call already
under way is refused too, so the switch takes effect at once.

The selection is stored per server, keyed on the name the agent sees, and
survives restarts. A change is announced, so the timeline records who narrowed
the tool surface and when. A tool the server later renames no longer matches, so
a stale entry disables nothing rather than the wrong thing.

### Derived provider
A provider name that is not a service itself but a second, separately scoped
connection to one. An example is a health-only connection on Google's endpoints
under its own name, held apart from the everyday *connected account*. Some APIs
require this, refusing any token that carries unrelated scopes.

It gets its own *credential* and connected-account row, and runs on the base
provider's endpoints. Aliases are ad hoc, so a derived name is absent from the
*OAuth provider registry* and never guessed from its spelling. The Connect form
asks which known provider it runs on, then fills in those endpoints and keeps
your name.
See also: *connected account*, *OAuth provider registry*.

### Delete (a thread)
Permanently removing a *thread*, its *sub-threads*, everything said in them, and what Lucidos learned from them. *Archive* only moves a thread to the Archive section, still fully searchable. Delete cannot be undone, though an earlier backup still holds it. Code already applied from a *coding-agent thread* stays; unapplied branch work goes, with the thread's *worktree* and branch. The thread's own *widgets* go too, in one commit. *Reusable widgets*, apps made from a widget, and other threads' widgets stay.

Offered to the workspace owner only, in the thread's own ⋯ menu. **Never to the *Lucidos Agent* or a *coding-agent thread***: no tool, CLI verb or SDK method exists. The route refuses any caller that is not a signed-in device, even one carrying the owner's *standing instruction*.

It cascades like *cascading archive*, with the same refusal: no member may be running, waiting on an answer, or holding a pending *change*. One confirmation names how many threads go. It also offers **Archive instead**, wherever Archive is still available.

Distinct from discarding a draft, which throws away a thread that was never sent.
See also: `docs/adr/0192-thread-delete-is-the-one-sanctioned-removal.md`.

### Domain event
An *event* the workspace itself emits via the `events(action="emit")` LLM tool or `lucidos events emit` CLI: anything observable about the user's world (`MorningRoutineCompleted`, `JobListingFound`, `PanasonicHeatpumpAdjusted`). Persisted with the inner event type (not the literal string `"DomainEvent"`). It always flows through the trigger matcher, so a *trigger*'s `on_event:` can subscribe to any domain event name. Persisted `ThreadEvent` variants are also subscribable, except the per-token streaming ones and the side-question events. See *scheduler blocklist* (dev).

**The name must be your own.** Every engine event name is refused, `engine::event_bus::SystemEvent` and `ThreadEvent` alike (including legacy spellings like `Thinking`). A domain event's `aggregate_id` is its event TYPE, where a thread event's is a thread uuid. So a borrowed name writes a permanent row that breaks any query reading the name as an id.
See also: `system-knowhow/thread-events/one-table-two-enums.md` § "Today the scheduler uses a blocklist", the Event APIs section of `.claude/rules/rust.md`.

### Dynamic bars
On a phone, the header, the thread title and the prompt glide away as you scroll down a thread. The header also glides away over an app that loads the SDK, once your finger lifts. They glide back as you scroll up or swipe to another pane. The header returns near the top of the thread, the prompt near the end, and the prompt stays put while you type. A reply never moves them, even while the follow toggle carries you through it.

The installed iOS app always runs dynamic bars, whatever the setting says: iOS keeps its status bar clear there, so the header runs behind it with its controls lowered 24px. Elsewhere it is on by default, and set in Settings > Appearance & Behavior > Mobile or the `mobile_dynamic_bars` preference.

### Endpoint catalog
The knowhow half of a *derived proxy entry*: a `data/knowhow/<name>-api.md` file listing the endpoints observed on a site, each with its params, response shape and quirks. The `apis.json` entry beside it is pure transport and names no paths. Without the catalog the LLM knows only that a proxy exists, so a derivation that emits one without the other has failed. It never records the user's own rows, only field names and types.
See also: *derived proxy entry*, `system-knowhow/deriving-an-api-from-a-site.md`.

### Derived proxy entry
A proxy entry obtained by watching a site's own frontend, for a site with no usable public API. The user drives the site once in a visible browser while its calls are captured. The result is an `apis.json` entry plus an *endpoint catalog*. It replays the user's own authenticated session and never bypasses anything: a CAPTCHA or bot wall stops it. Any secret found during capture goes to the engine's credential store, never into either artifact.
See also: *endpoint catalog*, *credential*, `system-knowhow/deriving-an-api-from-a-site.md`.

### Engine
The process serving one *workspace*: it holds the threads, answers the app, runs triggers and scheduled tasks, and talks to its database. One per workspace, started by the *workspace gateway* (dev term) and addressed through it. "Cannot reach the dev engine" means this client got no answer from that process.

The workspace is not gone, and the engine may be healthy: a client can lose the route to it. The picker and the Lucidos menu's Workspaces row reach the gateway instead, so listing and switching keep working. Settings → System → Overview shows its version and restarts it.
Contrast with *Lucidos Engine*, the actor chip on work the engine did without the LLM.

### Environment variable
A user-managed, **non-secret** `NAME=value` pair (Settings → System → Environment Variables). Lucidos injects it into every subprocess it spawns: `run_bash`, `run_python`, background tasks, scheduled scripts, *triggers*, and *coding agent* sessions. Examples: `CLAUDE_CODE_USE_VERTEX`, `LUCIDOS_REPO`, build flags, default model names.

Stored in the `environment_variables` table. Edited in Settings or by the *Lucidos Agent* via the grouped `env_vars` tool (`list` / `set` / `delete`; `set_environment_variable` is a back-compat alias for `set`). Applied per spawn, so a change takes effect on the next tool call or agent turn.

Distinct from a *credential*: env vars are non-secret and appear in tool-call payloads, logs, and the *event* store. Credentials hold secrets and feed the proxy auth pipeline. Names are uppercase letters, digits and underscores, not starting with a digit. They may not clobber engine-owned names (`CRED_*`, `OAUTH_*`, `PATH`, internal `LUCIDOS_*`), which always win a collision.

One engine-owned name reaches the same subprocesses: `LUCIDOS_DATABASE_URL`, a read-only connection URL for the workspace database. It cannot read stored secrets. Use `psql "$LUCIDOS_DATABASE_URL"` to read. It writes no table: change state through the `lucidos` command for it. The engine sets no `PG*` variables, so bare `psql` reaches nothing.

A credential can take a custom env var name, so its secret also injects as, say, `GITHUB_TOKEN`. That is an extra alias **in addition to** `CRED_<NAME>`, which keeps working. Either is set only for a *secret mention*. An auth handshake script never gets the custom name, only `CRED_*` and `OAUTH_*` names.

The engine also copies every pair into its own process environment once at startup. So a variable the engine itself reads picks up a change only on the next engine start.

<!--gloss-event-start-->
### Event
A past-tense fact about something that happened in the workspace, named in the past tense even when transient. Persisted events are written to the `events` table, replay, drive projections and match triggers. Transient events go over SSE only and never reach projections or the trigger matcher. Subtypes: thread lifecycle events (`MessageReceived`, `ResponseGenerated`, …), system events (notifications, preferences, …), and *domain events*. There is no *command* concept: anything imperative becomes a request event (`AppUiRefreshRequested`, not the legacy `RefreshAppUI`), and a subscriber chooses whether to act.
<!--gloss-event-end-->

### File preview modal
A read-only view of one file, drawn over whatever the *content pane* shows, without navigating. An *app* opens it through `lucidos.ui.previewFile`, and a file link in an agent's reply opens it too. A reader following a citation glances at the file and keeps their place.

A link's `#L510-L520` suffix, or a `:510-520` ending its label, names the lines. It takes the same locators and `line` / `line_end` as the `file` navigation target (a workspace data path or `repo:<repoId>:file:<path>`). It shows the same highlight and line numbers as the content pane's preview, plus a link that opens that full preview. Esc, a click outside, or its close control dismisses it. The *content pane*'s file preview IS a navigation: it replaces the pane's view and lands in the Back history.
See also: `system-knowhow/js-sdk/lucidos-threads-thread-management.md` § lucidos.ui.

