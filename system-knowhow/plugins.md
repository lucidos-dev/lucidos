---
name: Plugins
description: Use when the user wants to author, package, publish, share, install, update, or uninstall a Lucidos plugin: "build a plugin", "publish a plugin", "share this app as a plugin", "package this for other workspaces", "ship a knowhow bundle", "install the X plugin", "update plugins", "make a .lucidos-plugin".
---

<!-- domain-event: PluginSetupCompleted: Plugin setup emits this domain event after completing its procedure. -->

# Plugins

How to package a coherent bundle of workspace content (apps, knowhow, triggers, event hooks, scripts) so another workspace installs it as a unit. It also covers distribution, updates and uninstall. The v1 design is `docs/plans/2026-04-29-plugins-v1-design.md`. For setup right after install, see `plugin-setup.md`.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/plugins/when-a-plugin-is-the-right-artifact` | When a plugin is the right artifact; Questions to settle with the user before bundling; Plugin layout; Shipping event hooks; `manifest.toml` schema; Plugin media |
| `system-knowhow/plugins/shipping-widgets` | Shipping widgets; Artifact provenance; Install confirmation panel (A release can ask for setup to run again) |
| `system-knowhow/plugins/the-plugins-panel-and-marketplaces` | The Plugins panel and marketplaces |
| `system-knowhow/plugins/authoring-a-marketplace` | Authoring a marketplace (Repo layout and how the scanner finds plugins; Install URLs are generated, not authored; Registering and the scan cycle; Private and internal repos; Org permissions can block repo creation independently); Shipping triggers (auto-registration); What doesn't belong in a plugin; Where a plugin keeps its runtime state |
| `system-knowhow/plugins/three-distribution-shapes` | Three distribution shapes (1. Single-repo plugin: plain git URL; 2. Monorepo with subpath: GitHub tree URL; 3. Local archive: `.lucidos-plugin` file); Authoring loop; Versioning and updates (Affected triggers); Uninstall semantics |
| `system-knowhow/plugins/local-modifications-the-modified-badge` | Local modifications (the "Modified" badge) (An update keeps your changes; Proposing your patch upstream); Events emitted (`PluginInstalled`; `PluginLocalChangesMerged`; `PluginUninstalled`); Common mistakes to avoid; Diagnostic evidence |
