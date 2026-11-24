---
name: Lucidos Glossary, Core terms (continued)
description: Part of Lucidos Glossary. Its index lists the sections it covers.
---

<!-- domain-event: SetupInterviewCompleted: The setup-interview knowhow emits this domain event when the interview finishes. -->
<!-- domain-event: PluginSetupCompleted: Plugin setup emits this domain event after completing its procedure. -->

One part of `system-knowhow/glossary`, which lists every part.

## Core terms (continued)

### Plugin modified state
Whether a *plugin*'s shipped content was edited locally since install: the user, the *Lucidos Agent* or a *coding-agent thread* changed an app, knowhow, script or trigger it owns. Shown as a **Modified** badge on its plugin detail page, whose tooltip lists the changed paths. Updating merges those edits (see *plugin local patch*), and the install panel states the outcome per file. **Derived, not stored**: the engine diffs current `data/` against the install commit, so it self-heals on a revert and stays true after a merge. A file added in a plugin's app directory counts, build output aside; a new file in a shared root belongs to no plugin.

### Plugins panel
The top-level panel for discovering, installing and managing *plugins*, including the *apps* they ship. One list with an **Installed only** checkbox, **checked by default**. Checked, it shows every plugin on disk, app-bearing or not. Unchecked, it widens to the whole catalog of registered *marketplaces*, installed and available. A live search and a per-category filter compose with it.

Each plugin is a card that leads with its icon and opens its *plugin detail page* on a tap. Its one button runs **Install** (or **Update**) → **Setup** → **Open**, and **Uninstall** is on the detail page. "Setup" shows while the plugin's *setup thread* runs; "Open" launches its app once setup is done or if there was none. Installs and uninstalls go through the standard plugin confirmation panels. The catalog re-scans whenever shown (the panel opens or **Installed only** is unchecked). The engine never silently updates a marketplace plugin: it notifies, and the user updates from the card (see *Marketplace*).

With no marketplaces registered, the catalog and Settings → Marketplaces empty states offer **Add the official Lucidos marketplace** (registering `github.com/lucidos-dev/plugins`). Marketplaces are added and removed under Settings → Marketplaces.

Distinct from the *Apps* panel: a plugin's app appears in Apps (open it) AND its plugin appears here (manage, update, uninstall). Plugins that ship no app (knowhow-, trigger-, script- or auth-module-only) live here too, with links to each shipped file. See ADR 0019.

<!--gloss-proxy-start-->
### Proxy
The engine's route to an outside service. You connect the service once, as an entry in `data/config/apis.json`: an API key, an OAuth sign-in, or a custom handshake or signer. The *Lucidos Agent* calls it with `proxy_request`, an *app* with `lucidos.proxy(<name>)`, and a *script* with `lucidos proxy <name>`. A *trigger* reaches it through its thread or its script. The engine adds the *credential* to each request server-side, so an app never sees the key.

A *builtin provider proxy* reaches a model provider with no `apis.json` entry.
See also: `system-knowhow/building-an-auth-handshake.md`, `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`.
<!--gloss-proxy-end-->

### Preference
A single key→value user setting in the `preferences` table: theme, language, timezone, push notifications, the welcome message, chat model, UI scale, font, and so on. Most of what the user calls **Settings**; the rest (*models*, *credentials*, MCP servers, *repositories*) have their own stores. A preference is **global** (workspace-wide) or **device-scoped** (a per-device override that wins on the device that set it). The *Lucidos Agent* reads and writes the agent-settable ones with `preferences(action="get")` / `preferences(action="set")`; the human uses Settings. A write emits the persisted `PreferencesChanged` event (or `LanguageSet` / `TimezoneSet` for locale), which open pages live-apply.

Distinct from *config* (`data/config/` files like `apis.json`) and from a *credential*, a secret never stored as a preference. See `system-knowhow/preferences.md`.

### Pseudo-fullscreen
The *app UI* filling the whole viewport as a CSS overlay, where the native Fullscreen API is unavailable or refused. That is the iOS path, so it is usual on a phone. Both modes start from the same content-header control and look the same to the user.

In the desktop client it is always the mode. The window goes native fullscreen with it, so the app also covers the menu bar and Dock.

A natively fullscreen element is painted alone, so the browser supplies Escape and draws nothing else. A pseudo-fullscreen panel sits in the normal layer, so the host draws its own chrome over the app iframe. That is an exit button top-right, plus a transparent guard strip down each screen edge on mobile. Those regions belong to the host, and an app must keep them clear (`system-knowhow/building-an-app/scaffolding-defaults.md` § Responsive by default). Source: `.app-ui-fullscreen` in `crates/lucidos-app/src/styles/panels/previews.css`, gated by `isPseudo` in `components/apps/AppUiInline.tsx`.

### Pull to refresh
Dragging down past the top of a *content pane* view on a touch screen to re-read what it shows. A scroll up that keeps going past the top counts too. An arrow drops with the finger, blending into the accent colour, fully there once letting go would refresh. It then stops dropping but keeps turning while the finger pulls. A refresh icon spins beside the header's menu button until the new data lands, then turns into a check. On desktop it is the header's Refresh button.

Each view decides what it re-reads. Disk Usage re-measures its worktrees, the Plugins panel rechecks every marketplace, and an *app* reloads as its header Refresh does. Live views, such as the thread list, have nothing to pull. Inside an app the SDK sees the pull, and an app can opt out (`system-knowhow/js-sdk/remote-workspace-client-contract.md` § Pull to refresh).

### Pane swipe
On a phone, a sideways drag that moves between the three panes: the *thread drawer*, the *thread pane* and the *content pane*. A short fast flick or a drag past a third of the screen moves one pane; anything less springs back. It works anywhere, over an open *app* or an HTML preview too, except while an app is fullscreen. A swipe the phone interrupts, such as a notification taking the touch, springs back. Over a PDF or a web page preview it starts only at the screen edges. A carousel or slider in an app or HTML preview, or a clamped XL *widget box*, keeps its own sideways drag (`system-knowhow/js-sdk/remote-workspace-client-contract.md` § Pane swipe).

### One sound at a time
Starting a sound in one *app* or *widget* pauses every other sound playing in Lucidos, the file previews' players included. Two sound players on one question card never play over each other. Muted media takes no part. Inside an app the SDK handles it, with no code from the app (`system-knowhow/js-sdk/setup.md` § One sound at a time).

### PresenceCheck
The transient SSE event the *Lucidos Engine* broadcasts on every `NotificationCreated` to ask each connected page what it does with the notification. It **carries the toast** (`title`, `body`, `thread_id`, `event_id`, `app_id`, `tap`), plus `sent_at_ms` and a `deadline_ms`. The deadline comes from `scheduler::push::DEADLINE_MS`, currently 2 s. That covers an iOS PWA's first packet after Tailscale wakes from idle, which can take 1100–1800 ms.

Each page acts on it first, then answers with a *PresencePong* naming its *presence outcome*. The engine decides at the first pong that suppresses the push, or at the deadline, whether to send an *OS surface* push. The check is skipped only when nobody is reachable. That means no open SSE connection AND no device pinged visible within `PRESENCE_STALE_AFTER` (120 s, `core::device_presence`). The live SSE-connection count is the primary gate (`engine.sse_connections`); heartbeat candidates are secondary (`expected_pong_count` in `scheduler::push`).

The SSE count makes this robust. iOS suspends the 30 s heartbeat while a PWA is foregrounded, so the heartbeat row goes stale on a connected page. Gating on the open connection still lets that page suppress the push. See `system-knowhow/notifications/3-the-presencecheck-protocol.md` §3.

### PresencePong
The page's response to a *PresenceCheck*, POSTed to `/api/v1/presence-pong` with `notification_id`, `device_id` and its *presence outcome*. An OS push goes out iff NO pong reports an outcome that suppresses it; multi-tab pongs keep the strongest outcome per device. Late pongs (after the deadline) ack 200 and are dropped, since the race is normal; a late suppressing one is logged. One pong is owed per open SSE connection, so documents sharing one through the *shared SSE holder* are merged into a single POST. See `system-knowhow/notifications/3-the-presencecheck-protocol.md` §3.

### Presence outcome
What a page already did with a *PresenceCheck* when it answered. There are four values:
- `shown`: the toast is on screen.
- `auto_read`: the *source event* is on screen, so it marked the row read.
- `silenced`: toasts are off.
- `not_shown`: the shell is not in front of the reader.

Every value but `not_shown` suppresses the OS push. The page acts before it answers, so the push follows what the page did, never what it promised (ADR 0440). Rust `PresenceOutcome` (`scheduler::presence_outcome`), TS `PresenceOutcome` (`@lucidos/event-stream`). See `system-knowhow/notifications/3-the-presencecheck-protocol.md` §3.

### Shared SSE holder
The one `SharedWorker` per *workspace* per browser profile that owns that workspace's `GET /api/v1/events` connection. It relays every frame to its attached documents: the Lucidos shell, each app iframe, and each app in its own tab. So open apps no longer multiply connections. Its script URL (`/<slug>/api/v1/sse-worker.js`) carries the workspace prefix, so no document can receive another workspace's frames. It merges its documents' *PresencePong* answers into the one pong its connection owes, keeping the strongest *presence outcome*. A browser without `SharedWorker` (Chromium on Android, Android WebView) falls back to one private `EventSource` per document.

### NativePushRequested
The transient SSE event the *Lucidos Engine* emits to drive the native-desktop *OS surface*. Emitted **only on the push-ALLOWED branch**, when no page reported handling the *PresenceCheck*, with the same toast content. A desktop window the reader brought forward since it answered shows the in-app toast instead of the banner, so the frame always surfaces. A connected Tauri desktop app shows a macOS notification from it via its `show_native_notification` command. That uses Apple's `UserNotifications` framework (`UNUserNotificationCenter`), whose delegate captures the click to route the tap. WKWebView can't receive Web Push, so this is the desktop's web push, over the open SSE stream.

Needs a packaged `.app` build (inert in `tauri dev`). Browser / PWA pages ignore it (the handler gates on Tauri) and get the real web push on the same branch. See `system-knowhow/notifications/1-conceptual-model.md` §1, `system-knowhow/notifications/3-the-presencecheck-protocol.md` §4.

### Scratch
Ephemeral working files under `.lucidos/tmp/`, at the *workspace* root and so **outside** `data/`: gitignored, not indexed, not *artifact*s, safe to delete any time. `http_request(temp_path)` saves raw responses there, `git_clone` puts inspect-only checkouts there, and plugin archives are staged there during install. The file tools **read** it (`read_file`, and `copy_file` as a source to promote a file into `artifacts/imported/<name>/`).

They never **write** it, since they git-commit everything they write. So `write_file` / `edit_file` / `delete_file` refuse a scratch path and point at `run_python`, whose cwd is the workspace root. Only `.lucidos/tmp/` is addressable; the rest of `.lucidos/` (coding-agent *worktree*s, `exhaust/`, `engine.pid`) is refused both ways.
See also: `system-knowhow/best-practices.md` rules 8 and 10; ADR 0051.

### Spend cap hold
A pause in background model calls after a provider reports that its spending cap was reached.
It lasts until the provider's reset time, or one hour when no time was given.
The agent's own turns still send, and any successful call ends the hold.
**Try now** on the provider's settings row clears it by hand.
Each provider has its own hold; other providers keep working.

### Script
Code (Python, shell, JS) invoked by an *intent* or *knowhow*. Lives with its primary consumer when scoped (`data/apps/<id>/scripts/`, `data/triggers/<slug>/scripts/`, `data/knowhow/<domain>/scripts/`), or at top level (`data/scripts/`) when shared.

### Setup interview
A guided interview the *Lucidos Agent* runs to work out what this person should use Lucidos for. It ends with *app*s, *trigger*s and *knowhow* actually built in their *workspace* in that session.

Its first card asks how technical the person is, unless first-run setup already did, and stores the answer as their *technical literacy*. Every later card and thread uses that level. It is not work-only. The next card asks which parts of life to cover (work, home and personal admin, health and training, learning and side projects). It takes several answers, so a kit can center on training or a household as readily as a job.

Three entry points start it:

- the "Help me get the most out of Lucidos" button on the first-run welcome;
- the "Setup guide" row in the *Lucidos menu*, which the Lucidos mark opens on every viewport;
- the help button beside Search everywhere on the desktop header.

**It runs in the *home thread*,** wherever it starts, so it creates no thread of its own (ADR 0411).

The menu row says "guide", since "interview" reads as an interrogation to a newcomer. The two later entry points confirm first, since they send. Mobile has no header *button*: its header has no slot to spare for a rare action. There the menu row is the durable route, beside the welcome and simply asking. Every entry point sends the same ordinary message, so typing always reaches it.

`system-knowhow/setup-interview` drives it. That file owns the areas to ask about, the question ladder, which cards take several answers, and the answers-to-kit mapping. It also owns the confirm-before-building rule and what gets persisted. The record lives at `artifacts/setup-interview.md`, appended per run and never overwritten, plus a `SetupInterviewCompleted` *domain event*. Only facts the user stated reach memory or `user_profile.md`; the agent's conclusions stay in the artifact.

It is a sibling to the two other workspace-wide recipes:

- *workspace audit* asks whether the workspace matches current conventions;
- *workspace learning* asks whether the conventions match this user;
- the setup interview asks whether the workspace matches the person.

It is the only one of the three that needs the user present, since it asks and then builds rather than sweeping and proposing. Distinct too from a *setup thread*, which finishes installing one *plugin*.

### Setup thread
A *Lucidos Agent* *thread* the engine spawns when a *plugin* with a `setup` field is installed, from the *Plugins panel* or the `plugins(action="install")` tool. On an **update** it spawns only when the new `setup` differs from the installed one, or the author raised the *setup revision*. The user lands straight in it on install. Only a user-confirmed install spawns one; the background marketplace update check only notifies.

Its first message names the occasion. A first install seeds `Set up the newly installed <name> plugin.` An update seeds `Set up <name> again: its setup instructions changed since version <prior>.` under the title `Update <name> setup`, so the thread list never shows one name twice. A legacy record naming no version drops the `since version` clause, and says `this update changed its setup instructions` instead. A raised *setup revision* seeds `Set up <name> again: version <v> asks to run its setup again since version <prior>.` It adds the author's note when the manifest has a `setup_reason`.

The engine wrote that line, so it is attributed to the *Lucidos Engine* with the reason `plugin_setup`, never to the user. It records the plugin, both versions and the occasion. The message route popover shows "Plugin install" or "Plugin update", with the confirming device as "Confirmed on". See `system-knowhow/thread-events/wire-format-and-metadata.md` § Engine origins.

The agent loads `system-knowhow/plugin-setup` to plan the steps as a todo list. The author's setup instructions come by reference from the `PluginInstalled` event, not embedded in the thread. The agent walks the user through them, asking for credentials and choices and doing the wiring it can.

On an update it starts from the last run. It diffs the two `setup` texts. When only the *setup revision* rose, the diff is empty, so it re-runs the steps that install shipped files. It also reads the previous run's `PluginSetupCompleted` *domain event*, which holds the user's choices and what was skipped. Without one, the earlier setup thread is the fallback. A record is never enough alone: the agent checks each trigger, webhook, credential or config entry is in place, since a record cannot see a deletion.

Its id is in the `PluginInstalled` event, so the card's *Setup→Open* button can reopen it. The card treats setup as done once the thread is no longer `running` or `waiting_for_user_answer`. The same holds once the thread is gone entirely, with no summary row and no live *Thread Queue* entry. So a lost or stale setup thread degrades to *Open*, never a *Setup* button that errors.

### Setup revision
A positive integer, `setup_revision` in the *plugin manifest*. The author raises it to make an update run the *setup thread* again, although the `setup` text is unchanged. It exists for plugins whose setup installs a file outside `data/`, such as a Claude Code skill copied to `~/.claude/skills/`.

An update re-runs setup only when the staged revision is **greater** than the installed one. A missing field counts as 0. A lowered or removed one never re-runs setup, and a fresh install ignores it. An optional `setup_reason` says why, in one line of at most 200 characters. It shows in the confirmation panel and the thread's first message. Rules and the authoring step: `system-knowhow/plugins/shipping-widgets.md` § "A release can ask for setup to run again".

### Coding-agent grant
Whether a *trigger*'s runs may start a *coding-agent thread*, set per trigger under "Allow coding agents". The default is off: such a spawn is refused, and the run fails with a notification naming the trigger and the missing grant. It covers intent and script triggers alike.

Only the user grants it, on the trigger or by confirming a *batch*. An agent, an app frame or a plugin's `trigger.toml` cannot, though anyone may revoke it. An agent's edit to the trigger's work clears it. A granted trigger's coding agents hold every grant the user gave coding agents. So grant it only to a trigger whose input you trust. See `system-knowhow/triggers/grouping-triggers.md` § "Coding-agent grant".

### Program grant
An allowance for one program held by a *trigger*. A gated command fails the run when its program grant is missing. The notification opens a *permission request* where the user can grant the program and run again. Settings lists the trigger's active grants.

An agent can ask for program grants ahead of a run, when it creates or edits a trigger or with `lucidos ask-permission grant-trigger-programs`. The card names each program, and Allow grants exactly those.

Category grants ended on upgrade. In affected workspaces, **Check my workspace** includes the grants review in its Lucidos Agent thread. The agent asks one trigger per card, with the programs from its recorded runs.

### Folder grant
An allowance for a *trigger*'s sandboxed subprocesses to write in one folder outside the workspace.
An agent asks for it ahead of a run, when it creates or edits the trigger or with `lucidos ask-permission grant-trigger-folders`.
A blocked run also raises a *permission request* in its own thread, and Allow lets the run carry on.
Trigger details lists each one beside the *program grants*, and only the owner revokes one.
Where the *agent sandbox* cannot confine trigger writes, no folder grant is needed or offered.
See `system-knowhow/triggers/grouping-triggers.md` § Program grants for the boundary on each system and its gaps.

