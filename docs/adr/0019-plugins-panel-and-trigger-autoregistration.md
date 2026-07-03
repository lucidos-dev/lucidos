# 0019 — Plugins get their own panel; plugin triggers auto-register with provenance

- **Status** — Accepted
- **Date** — 2026-06-26

## Context

A *plugin* is a bundle of installable workspace content — any of `apps/`,
`knowhow/`, `triggers/`, `scripts/`, `auth-modules/`. But the only place to
browse, install, and uninstall plugins was the **Store** tab *inside the Apps
panel*, and the Apps panel's other tab (**Installed**) listed only *apps*. Two
problems followed:

1. **Non-app plugins had no home.** A knowhow-only or trigger-only plugin was
   installed from a panel literally labelled "Apps", and afterward appeared
   nowhere the user manages things — the Installed (apps) tab couldn't see it; it
   only showed as "Installed" back in the Store tab. The App Store had in fact
   *previously* been a separate top-level panel and was folded into Apps; that
   fold is what created this homelessness.
2. **Plugin triggers silently did nothing.** Install copies `triggers/<slug>/…`
   into `data/triggers/` but the scheduler is **events-only** — it replays
   `TriggerCreated`/`TriggerUpdated`/`TriggerDeleted` and never scans
   `data/triggers/`. So a plugin-shipped trigger file was inert until an
   install-time agent happened to call `create_trigger`, and even then nothing
   linked the resulting trigger back to its plugin (no provenance → uninstall
   couldn't clean it up → orphaned live triggers).

This ADR records the design settled in the grill thread that produced
`docs/plans/2026-06-26-plugins-panel-and-trigger-autoregistration.md`.

## Decision

1. **Un-fold the Store into a top-level `Plugins` panel** with **Installed |
   Store** tabs. Installed lists every installed plugin regardless of content;
   Store browses marketplaces. The **Apps** panel returns to a bare app list,
   keeping the marketplace chip + Update affordance on plugin-installed app rows.
2. **Plugins declare triggers in a per-trigger `trigger.toml`** inside
   `triggers/<slug>/` (mirroring how an app is a folder `apps/<id>/` with its own
   `manifest.json` — a trigger is a self-contained folder, not an entry in the
   plugin manifest). Plugin triggers are **event-driven only**; a declaration
   with a cron `schedule` is rejected at install.
3. **Install auto-registers** each declared trigger by emitting `TriggerCreated`
   stamped with a new `plugin_id` **provenance** field; triggers go **live
   immediately**, disclosed in the existing install confirmation panel (with
   their `side_effect_grant`). **Uninstall** auto-deletes every trigger carrying
   that `plugin_id`. **Update** re-syncs by `(plugin_id, slug)` — add new, remove
   gone, update changed.
4. **`trigger.toml` is a projection for *all* triggers**, not just plugin ones:
   EventBus materializes `data/triggers/<slug>/trigger.toml` from the trigger
   events as a **pure read-model** (events authoritative; hand-edits overwritten;
   rebuilt from events on restart). This removes the asymmetry where only plugin
   triggers would have an on-disk definition and keeps the plugin file from going
   stale after a UI edit.

## Rationale

- **Every plugin gets a home.** A dedicated Plugins panel is the one surface
  where a non-app plugin is visible, inspectable (content + shipped files), and
  uninstallable — the thing the fold-into-Apps removed.
- **Un-folding is not flip-flopping.** The earlier fold optimised for fewer
  panels; it had no answer for non-app plugins. The new reason to separate
  (give them a home) is strictly better-informed, hence this record.
- **App/trigger symmetry.** An app isn't declared in the plugin manifest — it's
  discovered by its folder. Triggers mirror that exactly (`triggers/<slug>/
  trigger.toml`), keeping the plugin manifest about plugin identity and each
  trigger self-contained with its knowhow/scripts.
- **Provenance is what makes lifecycle correct.** Without `plugin_id`, uninstall
  can't tell a plugin's triggers from the user's, so it either orphans them or
  risks deleting user triggers. The field is the minimum that makes
  auto-register, scoped-uninstall, and update-resync honest.
- **Events stay the authority.** The on-disk `trigger.toml` is a projection like
  `thread_summaries`/`notifications` — derived, rebuildable, never the source of
  truth — so it fits engine statelessness and the "git is the artifact store,
  never the authority" principle. Two-way (staged) file edits are deferred, not
  designed out.
- **Event-driven only in plugins.** Cron triggers are workspace state (when-on-a-
  clock), not reference material; only `on_event` triggers — part of the plugin's
  own mechanism — auto-register. This was already the documented expectation; the
  validation now enforces it.

## Consequences

- New top-level menu item `plugins`; the `app-store` navigate target now lands on
  Plugins → Store (older notifications still resolve).
- New offline endpoint `GET /api/v1/plugins/installed` (the `PluginInstalled`
  projection, with `content` + `files`) backs the Installed tab — it works
  without a marketplace scan and still lists a plugin whose marketplace was
  removed.
- `TriggerConfig` / `TriggerCreated` / `TriggerInfo` gain an optional `plugin_id`
  (`#[serde(default)]`, so legacy rows read fine; legacy triggers with no
  `plugin_id` are treated as user-owned and never auto-deleted by an uninstall).
- A new on-disk file (`data/triggers/<slug>/trigger.toml`) appears for every
  trigger. It is a **derived read-model**, NOT version-controlled: the engine
  adds it to the workspace repo's local `.git/info/exclude` and rebuilds it from
  events on boot. Engine-owned — a hand-edit is overwritten, never authoritative.
- This is delivered in phases (panel → categories → projection → auto-register);
  each phase is independently applyable.

## Alternatives considered

- **Keep the Store under Apps, just relabel.** Rejected: it leaves non-app
  plugins under a panel called Apps and doesn't give them a management home.
- **Declare triggers in the plugin manifest (`[[triggers]]`).** Rejected in
  favour of per-trigger `trigger.toml` for app/trigger symmetry, to keep the
  manifest about identity, and to avoid manifest bloat when a plugin ships
  several triggers.
- **Provenance only (no auto-register), or guide-only uninstall.** Rejected:
  provenance-only leaves inert trigger files that never fire; guide-only uninstall
  leaves live plugin triggers running after removal (the orphan problem). Triggers
  are engine-managed scheduler state, so auto-register + auto-delete is the
  consistent pair.
- **Two-way `trigger.toml` (file edits stage a change).** Deferred, not rejected:
  the pure read-model ships first (matches existing projections); a file-watcher +
  staging pipeline can layer on later without changing the projection direction.
