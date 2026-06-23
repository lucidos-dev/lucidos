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
  4. **Reuses Lucidos's shared component classes for controls** — `sdk-iframe.css` ships the same component layer the host shell uses, so the app looks like part of Lucidos instead of a bare HTML form: primary buttons are `<button class="action-btn">` (with `.action-btn-confirm` / `.action-btn-danger` variants, and `.action-btn-secondary` for a neutral secondary button beside a primary CTA), plus `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state` (full table in `js-sdk.md` § Component classes). A plain unclassed `<button>` does **not** match Lucidos's blue primary button — reach for `.action-btn`, and **don't hand-roll an outlined "secondary" button** — use `.action-btn-secondary`.

     **The `.action-btn` variants are *additive* — always keep the base class.** This is the standard base-plus-modifier shape (Bootstrap's `btn btn-primary`, BEM's `block block--modifier`): the base `.action-btn` carries *all* the geometry (padding, radius, sizing, font), and `-confirm` / `-danger` / `-secondary` only swap the color or fill. So every variant is `class="action-btn action-btn-X"`, never the variant alone:

     ```html
     <button class="action-btn">Save</button>                      <!-- primary (blue) -->
     <button class="action-btn action-btn-confirm">Apply</button>  <!-- green -->
     <button class="action-btn action-btn-danger">Delete</button>  <!-- red -->
     <button class="action-btn action-btn-secondary">Cancel</button> <!-- neutral outline -->
     ```

     The trap: `.action-btn` *does* work on its own — because the base **is** the primary button (unlike Bootstrap's neutral bare `.btn`). That's exactly why it's tempting to assume `.action-btn-secondary` also works alone. It does **not** — written by itself it matches no `.action-btn` rule and falls back to a plain grey browser button ("weird-looking button" bug). Pair it.
  5. **Sizes in `rem`, never `px`, so it respects the user's font size.** The user's UI-scale preference is applied as the root font-size, so only `rem`/`em` units scale with it — an app sized in `px` renders at a fixed size and ignores the setting (the "doesn't respect my font size" bug). Size text/spacing/radii in `rem` (or the `--space-*` / `--radius-*` tokens), matching Lucidos's type scale (body ≈ `0.8125`–`0.875rem`, small ≈ `0.6875rem`). `1px` borders are the only acceptable `px`. See `js-sdk.md` § "Respect the user's font size" for the token values.

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
        .row { display: flex; gap: var(--space-sm); margin-top: var(--space-md); }
      </style>
    </head>
    <body>
      <div class="panel">
        Hello
        <div class="row">
          <!-- Variants are additive — always keep the base `action-btn` class. -->
          <button class="action-btn">Primary</button>
          <button class="action-btn action-btn-secondary">Secondary</button>
        </div>
      </div>
      <script>
        lucidos.ui.applyPreferences();   // apply the user's theme on load
        lucidos.ui.watchPreferences();   // re-apply when it changes live
        // app code…
      </script>
    </body>
  </html>
  ```

  Opt out only for an app that ships its own complete visual identity (a game, a full-bleed chart canvas, an embedded third-party UI). For everything else, inheriting is the default — a workspace in light mode must never get a dark-only app.

## Visual quality bar — make it look sleek and native to Lucidos

The default for an agent-built app is **polished and visually indistinguishable from the host shell**, not "generic HTML form that happens to work". A cheap-looking app is a defect on the same footing as a broken one. The mechanics below are **non-negotiable**; the design principles after them are what separate "functional" from "a product designer would approve".

**Non-negotiable mechanics** (these are bugs if violated — full detail in *Scaffolding defaults* above and `js-sdk.md`):

1. **Inherit the theme.** Include the head scaffold (`/api/v1/sdk-prefs.js` → `/api/v1/sdk-iframe.css` → `/api/v1/sdk.js`, in that order) and call `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`.
2. **Zero hardcoded colors.** Every color is a theme variable — `var(--bg-primary)`, `var(--bg-secondary)`, `var(--text-primary)`, `var(--text-secondary)`, `var(--text-muted)`, `var(--border-color)`, `var(--accent)`, `var(--accent-green/yellow/red)`, `var(--shadow-sm/md/lg)`. A single hex literal is a bug — the light-mode workspace gets a dark-only app.
3. **Reuse the shared component classes.** `.action-btn` (+ the *additive* variants `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` — always `class="action-btn action-btn-X"`, never the variant alone), `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state`. A plain unclassed `<button>` does **not** match Lucidos's primary button — never hand-roll one.
4. **Size in `rem` / tokens, never `px`.** Use `rem` and the `--space-*` / `--radius-*` tokens for all text, spacing, and corners. `1px` borders are the only acceptable `px`. The user's font-size preference is the root font-size, so `px` ignores it.

**Design principles for a result that looks designed, not assembled:**

- **Generous whitespace.** Crowding reads as cheap. Pad containers with `--space-lg`; separate sections with `--space-xl`. When in doubt, add space, not borders.
- **Clear typographic hierarchy.** One obvious title (`.title` or an `h1`–`h6`), readable body (`0.8125`–`0.875rem`), quiet meta (`--text-muted`, `0.6875rem`). Size and color carry the hierarchy — don't bold everything.
- **Restrained color.** Build structure from `--bg-secondary` panels and `--border-color` hairlines. Reserve `--accent` for the **one** primary action per screen; `--accent-green/yellow/red` only for genuine status. A UI where everything is colored has no focal point.
- **One clear focal point per screen.** A single primary thing the eye lands on — one main CTA, one headline number, one list — not a dense grid of competing widgets. If two things both shout, neither does.
- **Consistent spacing rhythm.** Pick the space tokens and reuse them; don't mix `--space-sm` here and an ad-hoc `0.6rem` there. Even rhythm is most of what "feels designed".
- **Rounded corners + subtle depth.** `--radius-md` on cards/inputs/buttons (the host's default), `--radius-lg` for large surfaces. Lift a card off the background with `--shadow-sm`/`--shadow-md` — sparingly, not on every element.
- **Calm and confident over busy.** Fewer, better-spaced elements beat a wall of controls. Lean on Lucidos's defaults instead of inventing custom chrome — the more it looks like the host shell, the more native it feels.

### Smell test before finishing — self-apply every time

Before you call the app done, check it against this list. A "no" on any line means it's not finished:

- Does it follow **light *and* dark**? (Mentally flip the theme — would it still look right?)
- Any **hardcoded hex / `rgb()` / named color** anywhere? → replace with a theme variable.
- Any **`px` sizing** beyond `1px` borders? → convert to `rem` / `--space-*` / `--radius-*`.
- Any **plain `<button>`** (or a hand-rolled outlined button) instead of `.action-btn` / `.action-btn-secondary`?
- Does **every** `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` still carry the base `.action-btn`? A lone variant (`class="action-btn-secondary"`) renders as a plain grey browser button — the variants are additive modifiers.
- Only **real theme tokens**? The token set is closed — `--space-{xs,sm,md,lg,xl}`, `--radius-{sm,md,lg}`, `--font-ui` / `--font-mono` (full list in `js-sdk.md` § Theme variables). Don't invent `--space-2xl` or `--font-family` — an undefined `var()` silently drops the rule.
- Is there **one clear focal point**, generous whitespace, and a consistent spacing rhythm — or a cramped grid of equal-weight widgets?
- Dropped into Lucidos next to the host UI, **would it look like it belongs** — or like a bolted-on web page?
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

- **Chat path (quick edits)** — file tools (`write_file`, `edit_file`) on the live `data/apps/<id>/` files. Best for one-line tweaks, copy fixes, small CSS adjustments. The change lands immediately on workspace `main`. **You don't need to manually refresh:** when your turn finishes, the engine automatically refreshes every app you edited this turn — it reloads the open app UI (`AppUiRefreshRequested`) and refreshes the apps list (`AppUpdated`), once per app, coalesced (not per write). So just edit and finish; avoid spamming `refresh_app` mid-turn. (A brand-new app created this turn via `create_app` already appears via `AppCreated`.)
- **App coding-agent thread (heavier work)** — `run_coding_agent(folder='data/apps/<id>')` spawns a *coding-agent thread* in an isolated sparse-checkout *worktree* narrowed to that one app folder. Branch name shape: `claude-code/app/<id>/<ts>-<uuid>`. Best for multi-file refactors, new features, work that needs review before landing. Produces a *change* the user reviews + clicks *Apply*; Apply ff-merges to workspace `main`, emits `AppUiRefreshRequested` so open iframes reload, and does **not** restart the engine. `/harden` is not run for app changes — apps own their hardening (ship a `.claude/commands/harden.md` if you want one).

While the app coding-agent thread is open the user can preview the in-flight app via `?thread_id=<id>` on the app UI URL — the panel-overlay slot swaps from the live workspace copy to the WIP worktree copy. SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit live workspace data — data-coupled UI edits show their full effect only after Apply.

## Common mistakes to avoid

- **Storing data in `apps/{id}/`.** App data goes in `artifacts/{app-id}/`. The app code is git-tracked source; the data is user state. (See `best-practices.md`.)
- **Forgetting the `artifacts/` prefix in `lucidos.data.*` paths.** Paths are relative to `data/`, not `data/artifacts/`. App data lives at `artifacts/{app-id}/data.json` — *not* `{app-id}/data.json`. Without the prefix, `read` returns a 404 `SdkError` and `write` fails silently, which usually surfaces as "the checkbox toggles back" or "state doesn't persist".
- **Hardcoding colors / shipping a light-only (or dark-only) app.** New apps inherit the Lucidos theme by default (see *Scaffolding defaults*): include the theme assets, call `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`, and style with the theme CSS variables (`var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, …) instead of hex literals. Hardcoded colors ignore the user's OS light/dark setting — a light-mode workspace gets a jarring dark app, or vice versa. This is the single most common theming regression.
- **Sizing in `px` instead of `rem`.** The user's font-size / UI-scale preference is applied as the root font-size, so `px` values don't scale with it and the app ignores the setting. Size in `rem` (and the `--space-*` / `--radius-*` tokens); `1px` borders are the only acceptable exception. (See *Scaffolding defaults* item 5.)
- **Hand-rolling a secondary button — *or* writing `.action-btn-secondary` without the base class.** The shared layer ships `.action-btn` (+ `.action-btn-confirm` / `.action-btn-danger`) and `.action-btn-secondary`. Don't invent your own outlined/ghost button with `var(--accent)` — use `.action-btn-secondary`. But the variants are **additive**: write `class="action-btn action-btn-secondary"`, not `class="action-btn-secondary"` alone. A lone variant matches no `.action-btn` rule and falls back to a plain grey browser button — the "weird-looking secondary button" bug. (This is the standard base-plus-modifier shape, like Bootstrap's `btn btn-secondary`; see *Scaffolding defaults* item 4.)
- **Creating an app for a one-shot.** If the user only wants the answer once, just give it.
- **Inventing SDK calls — or guessing the surface, then checking later.** Load `system-knowhow/js-sdk.md` **before** you write the first SDK call or theme-CSS `var()`, not after the scaffold is written. The surface is small and stable but easy to misremember, and a scaffold written from memory tends to ship several guesses at once: e.g. there is no `lucidos.chat` namespace (`lucidos.chat.send` doesn't exist — open a chat with the typed `lucidos.ui.startThread({ prompt })`, the preferred wrapper over the low-level `lucidos.ui.navigate('new-chat', …)`), and `lucidos.data.read()` returns a **string** — `JSON.parse` it on read and `JSON.stringify` on write. Checking *after* you've written the JS catches the calls but tends to miss the CSS (the markup class / token errors hide in a later pass).
- **Hand-rolling the proxy URL with raw `fetch`.** Both `fetch('https://<external-host>/...')` (mixed-content / CORS will block it) and `fetch('/api/v1/proxy/<name>/...')` (same-origin so it runs, but constructs the helper's URL by hand and bypasses the SDK) are wrong. Use `lucidos.proxy(name).fetch(path, init)` and configure the backend in `data/config/apis.json` — one shape, no string-building.
- **Hardcoding credentials in iframe code.** The credential belongs in the engine's credential store, referenced by name from `apis.json`. The SDK never sees the secret.
- **Batching large rewrites.** After scaffold, prefer small visible changes the user can react to per round.
