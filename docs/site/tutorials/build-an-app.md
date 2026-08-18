# Build your first app

An **app** is a small, persistent UI you open again and again — a habit tracker, a
dashboard, a board. In Lucidos you don't scaffold one by hand: you *describe* it to
the Lucidos Agent in chat, it builds a working version in seconds, and you iterate by
talking. This tutorial walks through that loop end to end.

!!! info "Prerequisite"
    A running Lucidos workspace with an LLM provider configured — see the
    [Quickstart](../quickstart.md).

## 1. Describe what you want

Open a chat and say what the app is for, in plain language:

> "Make me a habit tracker — a few daily habits I can check off, and it should
> remember what I checked each day."

The agent first decides whether an app is even the right answer. A one-off question
("summarize today's emails") is just answered or saved to an artifact; a *recurring
UI* you'll come back to is what an app is for. If your request leaves the scope open,
expect at most a question or two — typically:

- **What's the smallest version that's useful?** (scope)
- **What should it remember between visits?** (storage)

Answer those and it moves straight to building — no long design phase.

## 2. It scaffolds a working version

The agent creates the smallest thing that demonstrates the idea: a working
`index.html` plus a `manifest.json` (the app's name, description, and icon). The app
shows up in your **Apps** panel and opens in the Canvas — the right-hand side on
desktop, a swipe away on mobile.

What you get by default is **native to Lucidos**, not a bare HTML form. Every new app:

- **Inherits your theme** — it follows your light/dark setting automatically, because
  the scaffold loads the SDK assets in order (`/api/v1/sdk-prefs.js`, then
  `/api/v1/sdk-iframe.css`, then `/api/v1/sdk.js`) and calls
  `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`.
- **Uses theme variables, never hardcoded colors** — `var(--bg-primary)`,
  `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, … so it looks right
  in both modes.
- **Reuses Lucidos's component classes** — `.action-btn` (with the additive
  `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` variants),
  `.list-row`, `.label`, and friends, so controls match the host shell.
- **Sizes in `rem`**, so it respects your font-size / UI-scale preference.
- **Works on your phone**, not just at desk width. The layout starts as one column
  and grows into more where there is room. Rows of controls wrap rather than
  overflow, and wide tables stack into cards. The agent also keeps the top-right
  corner free, because that is where Lucidos draws the exit button when you send an
  app fullscreen.

A minimal app the agent might start from looks like this:

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Habit Tracker</title>
    <script src="/api/v1/sdk-prefs.js"></script>
    <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
    <script src="/api/v1/sdk.js"></script>
  </head>
  <body>
    <div class="panel">
      <button class="action-btn">Add habit</button>
    </div>
    <script>
      lucidos.ui.applyPreferences();   // apply your theme on load
      lucidos.ui.watchPreferences();   // re-apply when you change it
      // app code…
    </script>
  </body>
</html>
```

## 3. Where the files live

An app is just files in your workspace under `data/apps/<id>/`:

```
data/apps/habit-tracker/
  manifest.json     # user-facing name, description, icon (NOT in the LLM's context)
  index.html        # the UI
  knowhow/          # optional: how-to docs the agent loads when the app is active
  intents/          # optional: stable statements of what you want
  scripts/          # optional: helper code
  triggers/         # optional: app-specific automation
```

The app's **code** lives here. The app's **data** lives separately, under
`data/artifacts/<id>/` — for the habit tracker, something like
`data/artifacts/habit-tracker/data.json`. Keep them apart: the code is versioned
source, the data is your evolving state.

The app persists state through the SDK rather than its own backend:

```js
// Paths are relative to data/ — so app data is "artifacts/<id>/...".
await lucidos.data.write('artifacts/habit-tracker/data.json', JSON.stringify(state));

const raw = await lucidos.data.read('artifacts/habit-tracker/data.json'); // returns a STRING
const state = JSON.parse(raw);
```

For anything that talks to an external service, the app calls
`lucidos.proxy(name).fetch(path, init)` and the credential stays server-side — never
pasted into the page.

## 4. Iterate by talking

This is the heart of the loop. To change the app, just say what you want:

> "Add a weekly streak count at the top."

For small edits the agent changes the files directly and the open app **reloads
automatically** when its turn finishes — you don't refresh anything. Prefer small,
visible changes you can react to each round rather than one giant rewrite.

For heavier work — a multi-file refactor, a real new feature — the agent can spawn an
**app coding-agent thread**: an isolated worktree narrowed to that one app folder.
That produces a reviewable *change* you **Apply** when you're happy with it (no
engine restart for app changes). While it's in progress you can preview the in-flight
version before applying.

## 5. The finishing bar

Before calling an app done, the agent self-checks that it looks like it belongs in
Lucidos: works in **both** light and dark, **no** hardcoded colors, **no** `px`
sizing beyond `1px` borders, real component classes instead of plain `<button>`s,
one clear focal point, and generous whitespace. A cheap-looking app is treated as a
defect, not a "good enough."

## Common pitfalls

- **Storing data in `apps/<id>/`.** App *data* belongs in `artifacts/<id>/`; the app
  folder is source.
- **Forgetting the `artifacts/` prefix** in `lucidos.data.*` paths — they're relative
  to `data/`, so it's `artifacts/<id>/data.json`, not `<id>/data.json`.
- **Hardcoding colors or sizing in `px`** — both ignore the user's theme and scale.
- **Using the browser's `alert` / `confirm` / `prompt` or a native `<select>`** —
  reach for `lucidos.ui.toast` / `confirm` / `prompt` / `Select`, which the host shell
  renders themed, above the app.
- **Creating an app for a one-shot.** If you only want the answer once, just ask.

## Going deeper

The authoritative engine guides are the source of truth for everything above:

- **Building an App** (`system-knowhow/building-an-app.md`) — when an app is the right
  fit, the visual-quality bar, and how to iterate.
- **JS SDK** (`system-knowhow/js-sdk.md`) — the full `lucidos.*` surface: `data`,
  `proxy`, `ui`, `events`, theme variables, and component classes.

Next: [automate part of it with a trigger →](automate-with-a-trigger.md)
