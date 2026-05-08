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

- **Inline `<script>` for small apps.** Split into `app.js` only when the script grows past ~100 lines or you want to share it with another script.
- **Inline `<style>` likewise.** External CSS is for shared design across apps.
- **Use the SDK for everything stateful.** Direct `fetch` to `/api/*` works but bypasses the workspace abstraction — `lucidos.data.read` / `lucidos.data.write` are the right primitives.
- **External APIs go through `lucidos.proxy`.** Apps load over HTTPS, so `fetch('http://...')` is mixed-content blocked and CORS blocks most cross-origin XHR. Add an entry to `data/config/apis.json` and call `lucidos.proxy(name).fetch(path, init)` — the engine forwards server-side and injects the configured auth header. Never paste credentials into iframe code. See `system-knowhow/js-sdk.md` § `lucidos.proxy`.
- **Manifest description matters.** It's how the user finds the app in the launcher and how the engine LLM knows what the app is for. Write it like a one-line README, not a tagline.

## Updating an app

There is no `update_app` tool. After `create_app`, all changes go through `write_file` / `edit_file` on `apps/{id}/index.html` (and `manifest.json` when the name or description changes). Don't recreate the app to change one button.

## Common mistakes to avoid

- **Storing data in `apps/{id}/`.** App data goes in `artifacts/{app-id}/`. The app code is git-tracked source; the data is user state. (See `best-practices.md`.)
- **Creating an app for a one-shot.** If the user only wants the answer once, just give it.
- **Inventing SDK calls.** Always check `system-knowhow/js-sdk.md` before writing app JS — the SDK surface is small and stable, but easy to misremember.
- **Direct `fetch` to an external API from the iframe.** Mixed-content / CORS will block it. Use `lucidos.proxy(name).fetch(...)` and configure the backend in `data/config/apis.json`.
- **Hardcoding credentials in iframe code.** The credential belongs in the engine's credential store, referenced by name from `apis.json`. The SDK never sees the secret.
- **Batching large rewrites.** After scaffold, prefer small visible changes the user can react to per round.
