# Build your first app

An **app** is a small, persistent UI you open again and again, such as a habit
tracker, a dashboard, or a board. You *describe* it to the Lucidos Agent in chat,
and it builds a working version in seconds. You then change it by talking. This
tutorial walks through that loop end to end.

!!! info "Prerequisite"
    A running Lucidos workspace with an LLM provider configured. See the
    [Quickstart](../quickstart.md).

## 1. Describe what you want

Open a chat and say what the app is for, in plain language:

> "Make me a habit tracker: a few daily habits I can check off, and it should
> remember what I checked each day."

The agent first decides whether an app is the right answer. It answers a one-off
question ("summarize today's emails") directly or saves the result to an
artifact. An app is for a *recurring UI* you come back to. If your request leaves
the scope open, expect a question or two, typically:

- **What's the smallest version that's useful?** (scope)
- **What should it remember between visits?** (storage)

After you answer, it starts building.

## 2. It scaffolds a working version

The agent creates the smallest thing that demonstrates the idea: a working
`index.html` plus a `manifest.json` (the app's name, description, and icon). The
app shows up in your **Apps** panel and opens in the Canvas (the right-hand side
on desktop, a swipe away on mobile).

Every new app follows the Lucidos look by default:

- **Inherits your theme.** It follows your light/dark setting automatically. The
  scaffold loads the SDK assets in order (`/api/v1/sdk-prefs.js`, then
  `/api/v1/sdk-iframe.css`, then `/api/v1/sdk.js`) and calls
  `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`.
- **Uses theme variables, never hardcoded colors**: `var(--bg-primary)`,
  `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, … so it looks
  right in both modes.
- **Reuses Lucidos's component classes**: `.action-btn` (with the additive
  `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` variants),
  `.list-row`, `.label`, and others, so controls match the host shell.
- **Sizes in `rem`**, so it respects your font-size / UI-scale preference.
- **Works on your phone.** The layout starts as one column and adds columns where
  there is room. Rows of controls wrap, and wide tables stack into cards. The
  agent keeps the top-right corner free for the exit button Lucidos draws when
  you send an app fullscreen.

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

An app is a set of files in your workspace under `data/apps/<id>/`:

```
data/apps/habit-tracker/
  manifest.json     # user-facing name, description, icon (NOT in the LLM's context)
  index.html        # the UI
  knowhow/          # optional: how-to docs the agent loads when the app is active
  intents/          # optional: stable statements of what you want
  scripts/          # optional: helper code
  triggers/         # optional: app-specific automation
```

The app's **code** lives here. Its **data** lives under `data/artifacts/<id>/`,
for example `data/artifacts/habit-tracker/data.json`. Keep them apart: the code
is versioned source, and the data is your changing state.

The app persists state through the SDK:

```js
// Paths are relative to data/, so app data is "artifacts/<id>/...".
await lucidos.data.write('artifacts/habit-tracker/data.json', JSON.stringify(state));

const raw = await lucidos.data.read('artifacts/habit-tracker/data.json'); // returns a STRING
const state = JSON.parse(raw);
```

To call an external service, the app uses `lucidos.proxy(name).fetch(path, init)`.
The credential stays server-side.

## 4. Iterate by talking

To change the app, say what you want:

> "Add a weekly streak count at the top."

For small edits, the agent changes the files directly. The open app **reloads
automatically** when the agent's turn finishes. Ask for small, visible changes
that you can react to each round.

For heavier work, such as a multi-file refactor or a new feature, the agent can
spawn an **app coding-agent thread**. It works in an isolated worktree narrowed to
that one app folder. The result is a reviewable *change* that you **Apply** when
you are happy with it. While the thread runs, you can preview the in-flight
version.

## 5. The finishing bar

Before calling an app done, the agent checks that it:

- works in **both** light and dark
- has **no** hardcoded colors
- has **no** `px` sizing beyond `1px` borders
- uses real component classes instead of plain `<button>`s
- has one clear focal point and generous whitespace

The agent treats a cheap-looking app as a defect.

## Common pitfalls

- **Storing data in `apps/<id>/`.** App *data* belongs in `artifacts/<id>/`; the app
  folder is source.
- **Forgetting the `artifacts/` prefix** in `lucidos.data.*` paths. They're relative
  to `data/`, so it's `artifacts/<id>/data.json`, not `<id>/data.json`.
- **Hardcoding colors or sizing in `px`.** Both ignore the user's theme and scale.
- **Using the browser's `alert` / `confirm` / `prompt` or a native `<select>`.**
  Use `lucidos.ui.toast` / `confirm` / `prompt` / `Select` instead. The host shell
  renders them themed, above the app.
- **Creating an app for a one-shot.** If you only want the answer once, ask in chat.

## Going deeper

The engine guides are the source of truth for this page:

- **Building an App** (`system-knowhow/building-an-app.md`): when an app is the right
  fit, the visual-quality bar, and how to iterate.
- **JS SDK** (`system-knowhow/js-sdk.md`): the full `lucidos.*` surface (`data`,
  `proxy`, `ui`, `events`), theme variables, and component classes.

Next: [automate part of it with a trigger →](automate-with-a-trigger.md)
