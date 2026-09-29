---
name: update-lucidos-cc-models
description: Use when the Claude Code model picker looks wrong or a new Claude model ships. The picker is discovered from Claude Code itself; this covers checking discovery, the curated fallback file and its context-window overlay.
---

# The Lucidos Claude Code model picker

The Claude Code `/model` picker in Lucidos is **discovered from Claude Code
itself** (ADR 0325). Nobody resyncs it by hand any more. What still needs
hands is the curated fallback file, and the context windows it declares.

## Where the list comes from

- **Discovery** (`runtime/cc_model_discovery.rs`, probe in
  `runtime/claude_code.rs::probe_cc_models`). A cold Claude Code process, built
  like a session, gets one `initialize` control request and no prompt. Its
  reply's `models` array is the picker, in Claude Code's order, with its own
  labels and per-model effort tiers. The reply's account data is dropped.
- **Cache:** `<workspace>/.lucidos/cc-models.json`. Loaded at boot, refreshed
  in the background when missing, a day old, or when a session's handshake
  names a new Claude Code version.
- **Fallback:** `runtime/cc_menu_options.json`, served only until the first
  probe succeeds.

The picker is **exactly** the discovered list. A model Claude Code does not
list is not offered, even one it would accept. So a new Claude model reaches
the picker when Claude Code lists it, usually after `claude update`.

## When the picker looks wrong

1. Read `models_provenance` on `GET /api/v1/claude-code/commands`. `source`
   says `discovered` or `fallback`; `error` holds the last failed probe.
2. Read `.lucidos/cc-models.json` for what the last probe saw.
3. Check the engine log for `[CcModels]` lines.
4. A list that follows the user's pins is correct. `ANTHROPIC_DEFAULT_SONNET_MODEL`
   and friends change what `sonnet` resolves to, and the picker says so.
5. To force a re-probe, delete the cache file and restart the engine.

## The curated fallback file

`cc_menu_options.json` has two jobs now: the picker before discovery, and the
**context-window overlay** that discovery cannot supply. Keep its rows roughly
current, but its order and labels matter only on a fresh install.

- Rows run **newest version first**, `default` at the head and version-free
  aliases at the tail. `the_model_rows_run_newest_version_first` enforces it.
- The `reasoning_efforts` list is still the effort vocabulary for validation.
- `STATIC_MODEL_LABELS` in `store/thread-events/exchange.ts` names models a
  thread was pinned to but no picker offers any more. Add a label there when a
  model leaves Claude Code's list.
- Run `./scripts/test-engine.sh -- -- runtime::claude_code::` after an edit.

## Declaring a context window

`context_window` says what window a Claude Code session on that model actually
runs under. Lucidos infers 200k for most bare `claude-` ids, because 1M mode is
gated on our own `[1m]` suffix. Claude Code picks its own context mode, so
without the declaration a real 240k prompt rendered as "203k / 200k (100%)".

An alias is never followed to the model it runs, because a legacy id folds
onto its alias. A session reports its concrete model at Init, and that id finds
its own curated row. Three rules when you add one:

- **Declare it on a pinned id, never on an alias row.** `normalize_cc_model_id`
  folds old dated ids onto `sonnet` / `opus` / `haiku`.
- **Leave the `[1m]` rows absent.** The id-shape rule already answers 1M.
- **Leave it absent when you do not know.** Too low reads over 100%; too high
  hides the running-out warning.

Read it through `runtime::coding_agent_context_window`, never by reaching into
the option. The chat model registry (`llm/model_registry.rs`) is a separate
answer for Lucidos's own calls.
