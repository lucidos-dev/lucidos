---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Find bar
The bar that finds text in what a pane shows, and steps a highlight through the matches, scrolling each into view. Mod+F opens it on the focused pane, and Mod+F again or Escape closes it. It searches what is shown, not files on disk (Search Everywhere's Text section does that).
- **An *app*:** the header's search button. The app must load the SDK, which runs the search inside the app (`system-knowhow/js-sdk/setup.md` § Find in app).
- **A file preview:** the header's search button, on source, Markdown, CSV, slides, a diff and an HTML artifact. Not a PDF, an image or the editor.
- **The *transcript*:** "Find in thread" in the thread's title menu. It searches all the text the thread shows, older history included: messages and replies, question and permission cards, step rows, side questions and error cards. It does not search button labels or text only shown in a step's detail, such as tool output. Typing counts the matches. Enter jumps to the first one below where you are reading, unfolding whatever hides it.

Where nothing can be searched and no find bar is open, Mod+F keeps the browser's own find.

### Font catalog
Every font Lucidos offers, one entry each: its id, label, CSS stack, kind (`ui` for proportional, `both` for monospaced, `mono` for code only), group, source and license. The group is `sans`, `serif` or `mono`, read from the stack's final generic family, and Settings lists fonts by it. The source is `vendored` (served by the engine, works offline) or `device` (the device's own fonts). No font loads from the internet.

The `font-family` *preference* takes a font id or `theme`, which follows the *theme*'s suggestion. A theme may name any catalog font. The engine serves the catalog at `GET /api/v1/fonts`, followed by any *workspace font*. See `system-knowhow/themes/switch-themes.md` § Fonts.

### Form request
Something the agent put in front of the user to fill in or confirm: a credential form, a plugin install or uninstall panel, an email confirmation, or an OAuth authorization page. It is persisted, so it survives a reload or a dropped connection. It stays open until answered, cancelled, or replaced by something newer, and one `FormRequestResolved` records which. An open one shows in its thread with an Open button and leaves the thread's status alone. A form opens on every device, an authorization page on the device it names, and either opens in one window per device: the window showing its thread, else the one used last.
See also: `system-knowhow/thread-events/form-requests.md` § Form requests.

### Event address
How one event in the store is named to an agent, written `evt-<32 hex>`. The hex is the event's own id, so the address is stable forever and resolves to exactly one row. Every live tool result ends with the address of the `ToolCalled` behind it, and a resumed tool block carries the same string as its `tool_use_id`. Two readers take it: the `events` tool's `query` action with `event_id`, and a `[KEEP OPEN]` line in the *working understanding*. A bare uuid, hyphenated or simple, works too.

It lets the agent note where something lives before a sweep drops it, then read it back. So reading a tool call's address returns the pair, call then result, since the result is the half that went.

### Image handle
The stable address of an image already in a thread, written `img-<hex>`. Derived from the image's content, so it names the same picture for as long as it exists. Contrast `thread:N`, a *position* that renumbers whenever an earlier image turns up. Every tool taking an image reference (`view_image`, `save_thread_image`, `generate_image`'s `input_images`) accepts both.

The conversation history's image note shows it beside `thread:N`, and the label on images attached to the current message shows it too. So the agent copies a handle rather than counting, and can note one for a later turn.
See also: `system-knowhow/best-practices/environment-variables.md` § Images posted in a thread.

### Image size hint
A trailing `#<width>x<height>` on a markdown image's source, in image pixels: `![mockup](artifacts/x.png#1600x1200)`. The thread uses it to hold the picture's box before the picture loads, so a card or reply does not jump. `lucidos data write` prints it on the picture line, and the engine adds it to every workspace picture on a question card. Keep it when you paste the line. Any other fragment is left as written.

### Imported
The `data/imported/` directory where imported external repositories land (via `RepositoryImported` events). Treated as *artifacts*: content is flattened into the workspace's git tree, not kept as nested git repositories. Distinct from a *repository*, a separately registered external git repo that an *external-repo coding-agent thread* runs against.

### In-app surface
The notification surface inside the Lucidos UI: the bell badge (unread count, top bar) and transient toasts. Driven by the `NotificationCreated` SSE message on a connected page, which decides locally from its own visibility, focused thread, and viewport. Independent of the *OS surface*: a notification can hit either, both, or neither (when auto-marked read on the *source event*).

The `notification_toasts` preference (workspace-wide, on by default) silences toasts. The bell badge cannot be silenced, so the notification still counts and waits in the Notifications panel. Toasts off does not hand it to the *OS surface*: the device is present, so the push stays withheld.
See also: `system-knowhow/notifications/1-conceptual-model.md` §1, `system-knowhow/notifications/3-the-presencecheck-protocol.md` §4, `system-knowhow/preferences.md`.

### Intent
What the user wants, in their words: stable, non-technical prose the LLM could read back to the user without sounding like a script. Lives in `data/apps/<app>/intents/<name>.md` or, for triggers, the `TriggerCreated` payload's `run.intent` field. Any length; never imperative *how* verbs (hit, parse, retry, fall back), which belong in *knowhow*.
See also: `docs/taxonomy.md` § Intent vs Knowhow, `system-knowhow/intent-registry.md`.

### Knowhow
How to achieve an *intent*, in technical terms: API details, data formats, quirks, workarounds, fallbacks. Evolves every time Lucidos learns something new: the *Lucidos Agent* writes and updates it as it learns. Lives in `data/knowhow/<id>.md` (shared) or scoped to an app / trigger. Discovered at runtime by the LLM via the `load_knowhow` tool, matched by the file's frontmatter `name` + `description`. Every knowhow file is either a *knowhow doc*, listed for routing, or a *knowhow reference*.
See also: `system-knowhow/building-knowhow.md`, `docs/taxonomy.md` § Intent vs Knowhow.

### Knowhow doc
A *knowhow* file the engine lists for routing. Every thread's Know-how list names it, with its id and description, so the LLM can `load_knowhow` it. Placement decides which files count: `data/knowhow/` and the shared `~/.lucidos/knowhow/` list `<name>.md` and `<group>/<name>.md`. An app's or a trigger's own `knowhow/` lists `<name>.md` only, since the app or the trigger is already the group. Anything deeper is a *knowhow reference*.
See also: `system-knowhow/building-knowhow.md` § "Where the file goes".

### Knowhow reference
A knowhow file that belongs to one *knowhow doc*: a long endpoint table, a payload dump, an error matrix. It sits in a folder named after the doc, below the listed depth, so it takes no row in the routing list. Placement is the whole distinction. A reference keeps its full id and `load_knowhow` still reads it. Nothing routes to one, so the owning doc must name its id, and the workspace audit flags a reference no doc names.
See also: `system-knowhow/building-knowhow.md` § "Where the file goes", `docs/taxonomy.md` § "Knowhow: Docs and References".

### Last used device
The *device* of your newest action in the current turn: the message that started it, a prompt sent while it runs, or an answer to a question card. Start a turn on your phone and answer from your laptop, and the laptop is the last used device from then on. `navigate_ui` sends a file, app or page there unless the agent names another device, and a sign-in page opens there. `capture_app` reaches that device too unless the agent names another, and `refresh_app` reaches only that device. A turn with no device, such as a trigger run, has none, so a navigate reaches every device showing the thread.

The agent's `[USER DEVICE & PREFERENCES]` block names it from the same lookup, so the two agree. The block is a snapshot from when it was built. A coding agent gets it once per conversation. It gets it again when a device or preference changed, after the conversation compacts, or on a new conversation.

Not an *active device*, which is one showing Lucidos right now, asked fresh per notification. Several devices can be active, but a turn has one last used device.
See also: *device*, *active device*.

### Live child
A *child thread* that has not finished its turn, so its *parent thread* is still owed a result. It is running, waiting for your answer, or paused for a resume the engine promised. It may also hold an *event wait*, or wait in the *Thread Queue* for capacity. An idle or failed child is not live.

Live children are what the child cap counts: a thread may have `max_concurrent_children_per_thread` of them at the same time (*capacity policy*, default 10). A finished child frees its slot. A child *moved to top level* still counts while it is live, so a move cannot buy back a slot. A *child follow-up* is never refused at the cap. See ADR 0380.
See also: *child thread*, *event wait*, *capacity policy*.

<!--gloss-live-cocreation-start-->
### Live co-creation
The principle at the heart of Lucidos: you and the *Lucidos Agent* shape the whole living system, **data and presentation together**. You do it continuously and in place, with no build → deploy → observe gap. The *Conversation* and the *Canvas* are always live and a gesture apart: side by side on desktop, a swipe apart on mobile. The Canvas runs on the real workspace, since apps read live data through the SDK. So you research, build and iterate inside Lucidos in one motion, seeing real behavior at once. The Conversation↔Canvas back-and-forth is the surface of live co-creation; the depth is that one conversation reaches the entire stack.
<!--gloss-live-cocreation-end-->

### Loaded knowhow
The *knowhow docs* a thread's `[LOADED KNOWHOW]` block carries in full this turn. A doc joins on the turn after its load. It leaves after a few agent replies without a load, or on `unload_knowhow`. A second `load_knowhow` of a doc the block holds keeps it and returns no body. The Context viewer lists the set as its Loaded knowhow tier, with how long each doc stays.
See also: `system-knowhow/building-knowhow.md` § "Lifecycle: loaded while in use", ADR 0425.

### Local certificate
The certificate that lets every browser on the gateway's own machine open `https://localhost:<port>`. This user's own certificate authority signs it for loopback only, so another user's process here cannot pose as the gateway. It needs this user's trust once: the desktop app's **Trust** screen, `install.sh` in a desktop session, or `lucidos tls trust`. Only the gateway's own machine trusts it, which is why an SSH tunnel to `localhost` no longer works. Contrast with a *managed certificate*, which other devices trust.
See also: `system-knowhow/remote-access/the-three-routes.md` § The local certificate, `system-knowhow/lucidos-cli/subcommands-14.md` § `lucidos tls trust | forget`.

### Lucidos Agent
The LLM driving a *thread* on the user's behalf: chat responses, trigger-thread runs, sub-thread callbacks, anything the LLM authored. UI actor-chip label: "Lucidos Agent". Returned by `mcp::client::mcp_client_name(ActorMode::Agent)` in `crates/lucidos-engine/src/mcp/client.rs`. Contrast with *Lucidos Engine*.

### Lucidos Engine
The engine itself acting without LLM mediation: recovery sweeps, *hardening*, scheduler ticks, system-initiated cancellations. UI actor-chip label: "Lucidos Engine". Returned by `mcp::client::mcp_client_name(ActorMode::Engine)`.

### Managed certificate
The HTTPS certificate a gateway gets by itself from tailscaled, for this machine's tailnet name, when the tailnet issues certificates. The gateway serves it on its non-loopback listeners and refreshes it daily, with no setup and no restart. A certificate the operator names in `LUCIDOS_TLS_CERT` always wins over it. Shown by `lucidos tls status` and the HTTPS row on Settings → Access.
See also: `system-knowhow/remote-access/the-three-routes.md` § "Automatic: the gateway's own certificate", ADR 0600.

### Marketplace
A registered git repository (or GitHub tree URL) that the *Plugins panel* scans for installable *plugins*, shown when the **Installed only** filter is unchecked. Stored in `data/config/plugin-marketplaces.json`; added or removed under Settings → Marketplaces (or the `plugins(action="register_marketplace")` tool). It holds one plugin at its root or several plugin directories; GitHub subdirectories become GitHub tree install URLs.

The engine scans marketplaces at startup, after registration changes, and every five minutes. Registering, renaming and removing one are announced, so an open *Plugins panel* and Settings → Marketplaces update in place. A newer version of an installed plugin raises one deduplicated "updates available" notification and is never applied automatically. The user applies it from the *Plugins panel* (with **Installed only** unchecked).

### Max tool calls
How many tool calls the *Lucidos Agent* may make in one turn before the engine ends it. Set under **Settings → Models → Chat & triggers**; the `preferences` knowhow states its default and its bounds (`max_tool_calls`). It counts calls, not replies, and applies to *trigger* runs exactly as to chat. The bound is far above any real turn, so the cost in time and tokens is the user's call.

Reaching it is not an error. The turn ends with a message prefixed `[ENGINE-LIMIT]` that names the limit and links to the setting; any message continues it. That prefix is the only trustworthy signal, since the agent cannot see its own call count and will otherwise invent one. Only the user can change the cap, the backstop over the agent's own work. Distinct from the *command guard*, which judges whether one command is safe.

### MCP server
An outside program that offers the *Lucidos Agent* extra tools over the Model Context Protocol: a Slack server, a Jira server, a company's internal catalog. Registered per workspace and managed under **Settings → MCP Servers**, which lists each with its cost.

**Running and offered are two different things.** *Running* is a workspace state: the server's process is up. A running server's tools reach only the threads that enabled it (see *enabled MCP server*). Every thread sees one short line per configured server and enables one when it needs it. So a forty-tool server costs nothing in a thread that never enabled it.

**Nothing starts an MCP server when the engine starts.** A restart stops every server process, hence the label "Running, this session". The threads keep what they enabled: a thread's next turn starts its enabled servers again before its first step. A **stopped** server's tool list is cached from its last successful connect. The page states that cost conditionally, stamped with when the tools were last seen. A server never connected says so instead of showing zero.

In a thread that enabled it, every tool's definition rides on every request. Two levers: switch off a single *disabled tool*, or stop the server, which switches it off in every thread. The page also holds the allowlist behind the *MCP permission card*.

**With the command guard on, a start asks once per command line.** A server starts only after the user approved the exact command it runs. Clicking Start or Enable approves it, as does a yes on the permission card. A changed command, argument or env value asks again. See `system-knowhow/running-python/pick-the-right-tool.md` § The command guard.

**The servers one thread enabled share a quarter of what a request can carry.** Past that, each server keeps the tools that fit, in its own listed order. The rest are left out of that thread's requests, and the agent is told which. The page states the worst case, every server enabled in one thread, and warns before anything is cut.

**Enabling, starting or stopping one reaches a turn already running.** The agent can enable a server mid-answer and use its tools in that answer. Stop a server mid-turn and the agent's very next step is already without its tools.

A server whose stored id cannot be used on the wire shows as unusable and offers only Remove; nothing on it can be called. See *wire tool name* in the developer glossary.

### Enabled MCP server
An *MCP server* that a *thread* switched on for itself, so that thread's requests carry the server's tools. Enabled is a thread state. *Running* is the workspace state of the process, shared by every thread.

The *Lucidos Agent* enables a server with the `mcp` tool's `enable` action, which starts the server if it is not running. `start` from a thread enables it there too, and `disable` switches it off in this thread only. A *trigger* lists the servers its fires use (`mcp_servers`), so its thread has them from the first step. A *child thread* starts with its parent's enabled servers, and after that the two are independent.

It survives a restart: the thread's next turn starts its enabled servers again. **Stop** switches the server off in every thread, and **Remove** does the same. Distinct from a *disabled tool*, which is one tool switched off for every thread.

### MCP permission card
The approval card the *Lucidos Agent* shows before calling a tool on an untrusted *MCP* server. Same UI as the *command permission card*: Deny, Allow once, Allow for this thread, Always allow this tool, or Always allow this server. "Always allow" choices go in an editable per-workspace list under **Settings → Permissions**. Entries are per tool (`Mcp(<server>:<tool>)`) or whole server (`Mcp(<server>:*)`). The thread waits until answered. A call to a server with auto-approve set never shows it.

Lucidos imports the legacy `mcp-allowed-tools` file at boot and deletes it after saving the grants.

A *trigger* never shows this card either, because nobody is there to answer it. In a trigger thread, an MCP call runs silently only when the trigger lists the server in `mcp_servers`, or the server has auto-approve set. Any other call is refused, and the agent is told to stop using that server in this fire. The user gets a notification that names the trigger and the server.

### Memory choice
The modal a new workspace shows on its first open, to pick its *memory module*. It waits until an AI provider is set up and no *release notice* is open. Classic carries "Almost free" and Tree "Costs money". A panel shows what Tree costs a month for a light, a medium and a heavy user, with the compactor model picked there.

"Use Tree" starts Tree as Start Tree in Settings does. Closing the modal keeps the workspace on Classic. It never asks again, and Settings → System → Memory changes the answer any time.

