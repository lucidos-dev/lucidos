---
name: Building Knowhow
description: Use when writing or updating a knowhow file (API quirks, payload shapes, integration recipes, workarounds): whether knowhow is the right artifact, standalone vs app-scoped placement, and writing descriptions the engine LLM will actually pick.
---

# Building Knowhow

How to write a knowhow file the engine LLM will find and use. The intent/knowhow/script taxonomy and frontmatter shape live in `docs/taxonomy.md` and the engine system prompt. The standalone-vs-app-scoped placement rule lives in CLAUDE.md and `system-knowhow/best-practices.md`. Apply those, don't restate them. The doc-vs-reference rule below is this file's own, because it changes how you write the file.

## When knowhow is the right artifact

Knowhow captures *technical detail you'd otherwise re-derive*: API quirks, payload shapes, working examples, known failure modes. They don't change every day, but they would be wrong to forget.

| You're writing… | Right place |
|---|---|
| "Here's how the Oura sleep API responds" | Knowhow |
| "The user wants daily sleep summaries" | Intent |
| "Lucidos engine architecture overview" | `docs/`, not knowhow |
| "Today I noticed X" | The chat, not knowhow |
| "Workaround for the Panasonic API rate limit" | Knowhow |

A fact that is the same in every workspace may belong in `docs/` or `system-knowhow/` instead. A fact specific to *this* workspace's setup is knowhow.

## Where the file goes: a doc, or one doc's reference

A knowhow **doc** is a file the engine lists for routing. A file below the
listing depth is a **reference**: it belongs to the doc above it, and only that
doc reaches it. Placement decides which one you wrote.

| Root | Listed as docs | Anything deeper |
|---|---|---|
| `data/knowhow/` (and the shared `~/.lucidos/knowhow/`) | `<name>.md` and `<group>/<name>.md` | a reference |
| `data/apps/<id>/knowhow/` | `<name>.md` | a reference |
| `data/triggers/<slug>/knowhow/` | `<name>.md` | a reference |

So a doc in a group folder is fine (`lucidos-ops/release-process`), and a file
under an app or a trigger sits at that root. The app or the trigger is already
the group.

**Split the supporting material out when a doc has a lot of it.** A long
endpoint table, a payload dump, a matrix of error codes: each is worth keeping,
and none is worth a row in every thread's routing list. Put them in a folder
named after the doc:

```
data/knowhow/
  lucidos-ops/
    release-process.md        ← the doc, listed
    release-process/
      phase-table.md          ← a reference
      rollback-matrix.md      ← a reference
```

**The doc pulls its own references in.** Name each one and its full id in the
doc body, so a thread that loaded the doc knows what to call next:

```markdown
The phase-by-phase table is in `lucidos-ops/release-process/phase-table`.
Load it with `load_knowhow` before you start a release.
```

Nothing else routes to a reference. It stays loadable by full id forever, but if no doc
names it, nothing can find it.

## Lifecycle: load-once-stays-loaded

When the engine LLM calls `load_knowhow` on a doc, the body goes into the `[LOADED KNOWHOW]` block of every later turn's user message. The LLM does **not** need to call `load_knowhow` again for that id. A second call is a no-op: the loaded set is keyed by id, so the body is overwritten with itself. The engine restores the loaded set from events on restart, so the doc stays loaded across engine restarts. There is no auto-unload and no LRU: a loaded doc stays loaded for the thread's lifetime.

So write the body for a reader who has it in context for the rest of the thread. Don't structure it to be re-read each turn, and don't plan for eviction. The LLM Context Viewer shows loaded docs under the **Loaded knowhow** tier, inside the user-message group.

## Questions to settle with the user before creating

A new top-level knowhow file shows up in retrieval forever, so confirm before adding one. Skip a question only when the user has already answered it.

1. **Is this stable workspace knowledge, or a one-shot answer?** For a one-off, answer in chat or save it under `artifacts/`. Knowhow is for reuse across future threads.
2. **Top-level or app-scoped?** Both are listed for routing and loaded on demand with `load_knowhow`, never injected whole. Placement changes where the doc lives and how it is addressed. App-scoped (`data/apps/<id>/knowhow/<name>.md`) answers to the id `<id>/<name>`. A thread with that app open also sees it named in its own block. If the file only makes sense inside one app, scope it there.
3. **What phrases would the user say when this becomes relevant?** The engine LLM sees the `description` in every thread, so list the synonyms and keywords the user actually uses. Confirm them with the user: they know their own vocabulary.
4. **Augment instead of fork.** If a knowhow on the topic exists, propose updating it rather than creating a new file. Confirm with the user before splitting one knowhow into two.

A short append to a knowhow you already maintain in the current task needs no question. That is part of the work.

## The `description` field is for retrieval, not for humans

The engine LLM sees every knowhow doc's name and description in every thread. It picks what to load by matching those descriptions against the user's message. The body loads only when the LLM chooses to read it. A reference is not listed, so its description does no routing. Write the description as *what the user would be saying when this becomes relevant*, not as a tagline.

Bad:

```yaml
description: Panasonic Comfort Cloud integration
```

Good:

```yaml
description: API quirks, auth flow, and payload shape for controlling Panasonic heatpumps via Comfort Cloud. Load when the user mentions heatpump, varmepumpe, Panasonic, or temperature control
```

Specific keywords win. Spend 1–2 sentences here: they are what make the body discoverable.

## Augment, don't fork

If a knowhow on the topic exists, edit it. Don't create `panasonic-v2.md` or `panasonic-better.md`: that fragments retrieval, and the LLM loads the wrong one. The exception is a separate concern (e.g. `panasonic-auth.md` vs `panasonic-payloads.md`) when one file would grow unwieldy.

## Good knowhow content

- **Working examples**: actual API requests and responses, not abstract descriptions
- **Quirks and failure modes**: "the API returns 200 with `error: true` in the body when X"
- **Concrete payload shapes**: JSON snippets, not English summaries
- **Workarounds with the reason**: why the obvious approach fails

Avoid:

- Philosophy ("the user values reliability over speed"): that's user profile, not knowhow
- Restating intent ("the user wants to track jobs"): intents own that
- Documenting the obvious ("call the API to get data")
- Stale specifics ("this works as of last Tuesday"): date them or remove them

## Calling external APIs from a recipe

A recipe that calls an external HTTP API the workspace holds a credential for uses the engine proxy. Never raw `curl -H "Authorization: Bearer $CRED_..."` or pasted-in headers. The proxy injects the auth header server-side. The credential never appears in script source, args, env vars, log lines, or the LLM tool transcript. Surfaces by consumer:

- **LLM running a trigger or agent step**: the `proxy_request` tool, calling the `data/config/apis.json` entry by name.
- **Script (bash / Python) invoked by an intent or trigger**: the `lucidos proxy <name> ...` CLI (see `system-knowhow/lucidos-cli.md` § `lucidos proxy`).
- **App UI inside an iframe**: `lucidos.proxy(name).fetch(path, init)` (see `system-knowhow/js-sdk.md` § `lucidos.proxy`).

Configure the backend once in `data/config/apis.json` (schema in `system-knowhow/best-practices.md` § `config/`). Knowhow then names it instead of restating credentials.

**A model provider needs no entry at all.** Every model provider the engine holds auth for is a *builtin provider proxy*: `anthropic`, `local`, `openai`, `openrouter`, `typesafe`, `vertex` and `xai`. They are not in `apis.json`, and a recipe never asks for their key. Each default base already includes `/v1`, so a recipe writes `lucidos proxy openai /models`, never `/v1/models`. See `system-knowhow/lucidos-cli.md` § `lucidos proxy`.

## Writing knowhow during execution

Knowhow is your *living* memory. When a trigger or app run teaches you something (a quirk, a better approach, a failure mode), update the relevant knowhow before moving on. The engine prompt's `CONTINUOUS LEARNING` note licenses this. Creating a new top-level file still needs the user's confirmation, per § "Questions to settle with the user before creating".
