---
name: Building an App
description: Use when the user wants to build, scaffold, edit, or extend an app — phrases like "make me an app", "build a tracker", "dashboard for X", "habit app". Covers when an app is the wrong fit, what to clarify before scaffolding, and how to iterate.
---

# Building an App

How to guide a user from "I want X" to a working Lucidos app. File layout, SDK, and frontmatter rules live in `system-knowhow/best-practices.md`, `system-knowhow/js-sdk.md`, and `docs/taxonomy.md` — load those when you need them.

## When an app is the right answer

An app is a persistent UI the user opens repeatedly. Push back on "make me an app" if something simpler fits:

| User says | Better answer |
|---|---|
| "Notify me when my package ships" | Trigger, not app |
| "Summarize today's emails" | Just answer, or save to `artifacts/` |
| "Track my morning habits" | App — repeated UI interaction |
| "Show me a chart of last week's runs" | If one-off, render in chat. If they'll keep coming back, app. |

When unsure, ask: "Do you want to open this regularly, or is it a one-time thing?" — one question, then act.

## Questions to settle with the user before creating

Two questions max before scaffolding — pick the ones the user's request leaves open. Good ones:

- **What's the smallest version that's useful?** (Surfaces scope.)
- **What data does it need to remember between visits?** (Surfaces storage shape.)

Don't design on paper. Once the questions are answered, scaffold the smallest thing that demonstrates the idea (`create_app` with a working `index.html` + `manifest.json`), show it, then iterate.

## Scaffolding defaults

- **Inherit the Lucidos theme — every new app, by default.** Apps follow the user's light/dark (OS) appearance for free, exactly like the rest of Lucidos. There is no separate "theme" to configure — the platform exposes it, the app just consumes it. The default scaffold does three things:
  1. **Includes the theme assets in `<head>`** (full boilerplate in `system-knowhow/js-sdk.md` § Setup): `<script src="/api/v1/sdk-prefs.js"></script>`, then `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">`, then `<script src="/api/v1/sdk.js"></script>`.
  2. **Calls `lucidos.ui.applyPreferences()`** (applies the theme on load — resolves a `system` preference to the live OS light/dark) **and `lucidos.ui.watchPreferences()`** (re-applies when the user changes it).
  3. **Styles with the theme CSS variables — never hardcoded colors:** `var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, etc. (full list in `js-sdk.md` § Theme variables). These flip light↔dark automatically; hex literals do not.

  Minimal scaffold to start from:

  ```html
  <!DOCTYPE html>
  <html>
    <head>
      <meta charset="utf-8">
      <title>My App</title>
      <script src="/api/v1/sdk-prefs.js"></script>
      <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
      <script src="/api/v1/sdk.js"></script>
      <style>
        /* Theme variables only — these follow the user's light/dark setting. */
        .panel {
          background: var(--bg-secondary);
          color: var(--text-primary);
          border: 1px solid var(--border-color);
          border-radius: var(--radius-md);
          padding: var(--space-lg);
        }
      </style>
    </head>
    <body>
      <div class="panel">Hello</div>
      <script>
        lucidos.ui.applyPreferences();   // apply the user's theme on load
        lucidos.ui.watchPreferences();   // re-apply when it changes live
        // app code…
      </script>
    </body>
  </html>
  ```

  Opt out only for an app that ships its own complete visual identity (a game, a full-bleed chart canvas, an embedded third-party UI). For everything else, inheriting is the default — a workspace in light mode must never get a dark-only app.
- **Inline `<script>` for small apps.** Split into `app.js` only when the script grows past ~100 lines or you want to share it with another script.
- **Inline `<style>` likewise.** External CSS is for shared design across apps.
- **Use the SDK for everything stateful.** Direct `fetch` to `/api/v1/*` works but bypasses the workspace abstraction — `lucidos.data.read` / `lucidos.data.write` are the right primitives.
- **External APIs — call `lucidos.proxy(name).fetch(path, init)`. Always.** Configure the backend in `data/config/apis.json`; the engine forwards server-side and injects the configured auth header. Never paste credentials into iframe code. See `system-knowhow/js-sdk.md` § `lucidos.proxy`. The two wrong shapes that look right:
  - `fetch('https://<external-host>/...')` from the iframe — mixed-content / CORS will block it, and any credential is sitting in the iframe.
  - `fetch('/api/v1/proxy/<name>/...')` from the iframe — same-origin so it *runs*, but it's the proxy URL the SDK helper builds for you. Constructing it by hand makes the proxy name a magic string and skips any future SDK-side concerns (timeouts, retries, response parsing, error shape). Use the helper.
- **Manifest description matters.** It's how the user finds the app in the launcher and how the engine LLM knows what the app is for. Write it like a one-line README, not a tagline.

## Updating an app

There is no `update_app` tool. After `create_app`, all changes go through `write_file` / `edit_file` on `apps/{id}/index.html` (and `manifest.json` when the name or description changes). Don't recreate the app to change one button.

### Editing an app with a coding agent

Two paths, picked per request:

- **Chat path (quick edits)** — file tools (`write_file`, `edit_file`) on the live `data/apps/<id>/` files. Best for one-line tweaks, copy fixes, small CSS adjustments. The change lands immediately on workspace `main`; open iframes don't auto-refresh (user re-opens the app to see it).
- **App coding-agent thread (heavier work)** — `run_claude(folder='data/apps/<id>')` spawns a *coding-agent thread* in an isolated sparse-checkout *worktree* narrowed to that one app folder. Branch name shape: `claude-code/app/<id>/<ts>-<uuid>`. Best for multi-file refactors, new features, work that needs review before landing. Produces a *change* the user reviews + clicks *Apply*; Apply ff-merges to workspace `main`, emits `AppUiRefreshRequested` so open iframes reload, and does **not** restart the engine. `/harden` is not run for app changes — apps own their hardening (ship a `.claude/commands/harden.md` if you want one).

While the app coding-agent thread is open the user can preview the in-flight app via `?thread_id=<id>` on the app UI URL — the panel-overlay slot swaps from the live workspace copy to the WIP worktree copy. SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit live workspace data — data-coupled UI edits show their full effect only after Apply.

## Common mistakes to avoid

- **Storing data in `apps/{id}/`.** App data goes in `artifacts/{app-id}/`. The app code is git-tracked source; the data is user state. (See `best-practices.md`.)
- **Forgetting the `artifacts/` prefix in `lucidos.data.*` paths.** Paths are relative to `data/`, not `data/artifacts/`. App data lives at `artifacts/{app-id}/data.json` — *not* `{app-id}/data.json`. Without the prefix, `read` returns a 404 `SdkError` and `write` fails silently, which usually surfaces as "the checkbox toggles back" or "state doesn't persist".
- **Hardcoding colors / shipping a light-only (or dark-only) app.** New apps inherit the Lucidos theme by default (see *Scaffolding defaults*): include the theme assets, call `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`, and style with the theme CSS variables (`var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, …) instead of hex literals. Hardcoded colors ignore the user's OS light/dark setting — a light-mode workspace gets a jarring dark app, or vice versa. This is the single most common theming regression.
- **Creating an app for a one-shot.** If the user only wants the answer once, just give it.
- **Inventing SDK calls.** Always check `system-knowhow/js-sdk.md` before writing app JS — the SDK surface is small and stable, but easy to misremember.
- **Hand-rolling the proxy URL with raw `fetch`.** Both `fetch('https://<external-host>/...')` (mixed-content / CORS will block it) and `fetch('/api/v1/proxy/<name>/...')` (same-origin so it runs, but constructs the helper's URL by hand and bypasses the SDK) are wrong. Use `lucidos.proxy(name).fetch(path, init)` and configure the backend in `data/config/apis.json` — one shape, no string-building.
- **Hardcoding credentials in iframe code.** The credential belongs in the engine's credential store, referenced by name from `apis.json`. The SDK never sees the secret.
- **Batching large rewrites.** After scaffold, prefer small visible changes the user can react to per round.
