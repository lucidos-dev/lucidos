---
name: Building Knowhow
description: Use when writing or updating a knowhow file (API quirks, payload shapes, integration recipes, workarounds): whether knowhow is the right artifact, standalone vs app-scoped placement, and writing descriptions the engine LLM will actually pick.
---

# Building Knowhow

How to write a knowhow file the engine LLM will actually find and use. The intent/knowhow/script taxonomy and frontmatter shape are in `docs/taxonomy.md` and the engine system prompt — don't restate them. The standalone-vs-app-scoped placement rule is in CLAUDE.md and `system-knowhow/best-practices.md` — apply, don't restate.

## When knowhow is the right artifact

Knowhow captures *technical detail you'd otherwise re-derive*: API quirks, payload shapes, working examples, known failure modes. Things that don't change every day but would be wrong to forget.

| You're writing… | Right place |
|---|---|
| "Here's how the Oura sleep API responds" | Knowhow |
| "The user wants daily sleep summaries" | Intent |
| "Lucidos engine architecture overview" | `docs/`, not knowhow |
| "Today I noticed X" | The chat — not knowhow |
| "Workaround for the Panasonic API rate limit" | Knowhow |

If a fact is stable across users and would be the same in every workspace, it might belong in `docs/` or `system-knowhow/` instead. If it's specific to *this* workspace's setup, it's knowhow.

## Lifecycle: load-once-stays-loaded

When the engine LLM calls `load_knowhow` on a doc, the body lives in the `[LOADED KNOWHOW]` block of every subsequent turn's user message — the LLM does **not** need to re-call `load_knowhow` for the same id later in the thread. Calling it twice for the same id is a no-op (the loaded set is keyed by id; the second insert overwrites with the same body). The engine restores the loaded set from events on restart, so the doc stays loaded across engine restarts within the same thread. There is no auto-unload and no LRU; once loaded, a knowhow doc stays loaded for the whole thread's lifetime — matching Claude Code Skills (loaded once → persists for the session) and Codex AGENTS.md (re-sent each turn via stateless conversation history).

Practical implication for knowhow authors: write the body assuming it will be in context for the rest of the thread once it's been loaded. Don't structure it to be re-read on each turn, and don't worry about it being evicted partway through. The LLM Context Viewer surfaces loaded docs under the **Loaded knowhow** tier inside the user-message group so you can see exactly what's currently in context.

## Questions to settle with the user before creating

A new top-level knowhow file shows up in retrieval forever — confirm before adding one. Skip questions only when the user has already answered them.

1. **Is this stable workspace knowledge, or a one-shot answer?** If they just need the answer once, give it in chat or save it under `artifacts/`. Knowhow is for things that should be reused across future threads.
2. **Top-level or app-scoped?** Top-level (`data/knowhow/`) is workspace-wide. App-scoped (`data/apps/<id>/knowhow/`) is loaded only in threads that involve that app. If the file only makes sense inside one app, scope it there.
3. **What phrases would the user say when this becomes relevant?** The `description` is what the engine LLM sees in every thread — list the synonyms / keywords the user actually uses. Confirm them with the user; they know their own vocabulary better than you do.
4. **Augment instead of fork.** If a knowhow on the topic exists, propose updating it rather than creating a new file. Confirm with the user before splitting one knowhow into two.

A short append to a knowhow you're already maintaining in the current task does NOT need a question — that's part of the work (see "Writing knowhow during execution" below).

## The `description` field is for retrieval, not for humans

The engine LLM sees every knowhow's name + description in every thread (the body only loads when the LLM chooses to read it). It picks what to load by reading those descriptions and matching them against the user's message. Write descriptions in terms of *what the user would be saying when this becomes relevant*, not as a tagline.

Bad:

```yaml
description: Panasonic Comfort Cloud integration
```

Good:

```yaml
description: API quirks, auth flow, and payload shape for controlling Panasonic heatpumps via Comfort Cloud — load when the user mentions heatpump, varmepumpe, Panasonic, or temperature control
```

Specific keywords win. List the synonyms a user might actually use. The description loads in every thread; the body only loads when retrieval fires — so it's worth investing 1–2 sentences here to make the body discoverable.

## Augment, don't fork

If a knowhow on the topic exists, edit it. Don't create `panasonic-v2.md` or `panasonic-better.md` — that fragments retrieval and the LLM ends up loading the wrong one. The exception: a genuinely separate concern (e.g. `panasonic-auth.md` vs `panasonic-payloads.md`) when one file would otherwise grow unwieldy.

## Good knowhow content

- **Working examples** — actual API requests/responses, not abstract descriptions
- **Quirks and failure modes** — "the API returns 200 with `error: true` in the body when X"
- **Concrete payload shapes** — JSON snippets, not English summaries
- **Workarounds with the reason** — why the obvious approach doesn't work

Avoid:

- Philosophy ("the user values reliability over speed") — that's user profile, not knowhow
- Restating intent ("the user wants to track jobs") — intents own that
- Documenting the obvious ("call the API to get data")
- Stale specifics ("this works as of last Tuesday") — date them or remove them

## Calling external APIs from a recipe

If the knowhow describes how to call an external HTTP API the workspace owns a credential for, the recipe should use the engine proxy — not raw `curl -H "Authorization: Bearer $CRED_..."` or pasted-in headers. The proxy injects the auth header server-side, so the credential never appears in the script source, args, env vars, log lines, or the LLM tool transcript. Surfaces by consumer:

- **LLM running a trigger or agent step** — `proxy_request` tool. Same `data/config/apis.json` entry, called by name.
- **Script (bash / Python) invoked by an intent or trigger** — `lucidos proxy <name> ...` CLI (see `system-knowhow/lucidos-cli.md` § `lucidos proxy`).
- **App UI inside an iframe** — `lucidos.proxy(name).fetch(path, init)` (see `system-knowhow/js-sdk.md` § `lucidos.proxy`).

Configure the backend once in `data/config/apis.json` (schema in `system-knowhow/best-practices.md` § `config/`); knowhow then references it by name instead of restating credentials.

## Writing knowhow during execution

Knowhow is your *living* memory. When you discover something new while running a trigger or app — a quirk, a better approach, a failure mode — update the relevant knowhow before moving on. The engine prompt's `CONTINUOUS LEARNING` note is the explicit license to do this. Confirm with the user before creating a new top-level knowhow file (it shows up in retrieval forever); appending to one you're already maintaining for the task at hand is part of the work.
