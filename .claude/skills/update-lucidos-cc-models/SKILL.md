---
name: update-lucidos-cc-models
description: Use when updating the hardcoded Lucidos CC model list — checks current CC /model picker and updates the Rust constant
---

# Update Lucidos CC Model List

The Lucidos CC model picker uses a hardcoded list of models matching Claude Code's `/model` picker.
This skill helps keep that list in sync when Anthropic updates available models.

## Source of Truth

The canonical model list comes from:
1. **Claude Code docs**: https://code.claude.com/docs/en/model-config
2. **Running `/model` in Claude Code** (interactive TUI — shows the live picker)
3. **CC system init event**: the `model` field shows the current default

CC does NOT expose available models programmatically (see github.com/anthropics/claude-code/issues/12612).

## Where the List Lives

**File**: `crates/lucidos-engine/src/runtime/cc_menu_options.json`
**Loaded by**: `cc_model_options()` / `cc_reasoning_effort_options()` in `claude_code.rs` (`include_str!` + `LazyLock`).

Each entry has:
- `value` — the alias CC accepts (e.g., `"sonnet"`, `"opus"`, `"haiku"`)
- `label` — display name (e.g., `"Sonnet 4.6"`)
- `description` — one-line description (e.g., `"Best for everyday tasks"`)
- `context_window` (optional, tokens): see "Declaring a context window" below

The JSON file also carries the `reasoning_efforts` list (`/effort` picker entries) under the same schema.

## Update Procedure

1. **Open the JSON file** `crates/lucidos-engine/src/runtime/cc_menu_options.json`.
2. **Compare with CC's picker**: run `claude` interactively and type `/model`, or check the model-config docs page.
3. **Edit the JSON**: add/remove/modify entries to match. No Rust source touch required.
4. **Mirror the change** into `crates/lucidos-app/src/api/client/chat.ts` (`CodingAgentModelValue`) and `crates/lucidos-app/src/store/thread-events/exchange.ts` (`STATIC_MODEL_LABELS`). Both are hand-maintained, with no codegen. A value added or removed in the JSON must reach the union; a label change must reach the fallback map. Do both in the same commit.
5. **Run tests**: `cargo test -p lucidos-engine --lib -- cc_model` — `command_definitions_include_model_options` validates that the standard aliases stay present.
6. **Commit**: `fix: update CC model list to match current /model picker`.

## Known Aliases

CC accepts these short aliases for `set_model` control requests:
- `default` — tier default
- `sonnet` — latest Sonnet
- `opus` — latest Opus
- `haiku` — latest Haiku

**An alias resolves per provider, and it moves.** On the Anthropic API (what
Lucidos spawns against) `sonnet` resolves to **Sonnet 5** and `opus` to **Opus 5**
as of CC v2.1.219; on Bedrock / Google Cloud those same aliases land on older
versions. So an alias row's *label* goes stale silently whenever Anthropic
repoints it. Two consequences for this file:

- Re-check what each alias resolves to on every resync (the model-config docs
  page has the per-provider table), and fix the label if it moved. The `opus` /
  `opus[1m]` rows once stayed at "Opus 4.6" past that point. Both rows, and
  their `STATIC_MODEL_LABELS` mirror, are now version-free like `sonnet`.
- Prefer a **pinned full id** (`claude-sonnet-5`, `claude-opus-5@default`) for the
  models the picker recommends, and keep an alias row only where "always latest"
  is the point. A pinned id cannot drift.

For 1M context variants, use the full model ID with extended context flag. Note
that a `<model>[1m]` alias is a no-op once the alias already resolves to a model
with a native 1M window, which is why the picker carries no `sonnet[1m]` row.

## Declaring a context window

`context_window` says what window a CC session on that model actually runs
under. It exists because Lucidos infers 200k for any bare `claude-` id: 1M mode
is gated on our own `[1m]` suffix, which is true of the requests the ENGINE
makes and false of CC's. CC picks its own context mode. Without the
declaration, the LLM Context Viewer rendered a real 240k Sonnet 5 prompt as
"203k / 200k (100%)".

Three rules when you add a model:

- **Declare it on a pinned id, never on an alias row.**
  `normalize_cc_model_id` folds old dated ids down onto `sonnet` / `opus` /
  `haiku`. A 1M declaration there is wrong for every Sonnet 4.6 session already
  in the store.
- **Leave the `[1m]` rows absent.** The id-shape rule already answers 1M for
  them, and a second copy of the number is a second thing to keep in step.
- **Leave it absent when you do not know.** Absent means the models registry
  answers, which is today's behaviour. Both directions cost something: too low
  and the bar reads over 100%, too high and the user loses the running-out
  warning.

Read it through `runtime::coding_agent_context_window`, never by reaching into
the option. The models registry (`llm/model_registry.rs`, the `models` table)
is a separate answer for Lucidos's own calls, and it must not move.
