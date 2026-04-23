---
name: update-cognos-cc-models
description: Use when updating the hardcoded CognOS CC model list — checks current CC /model picker and updates the Rust constant
---

# Update CognOS CC Model List

The CognOS CC model picker uses a hardcoded list of models matching Claude Code's `/model` picker.
This skill helps keep that list in sync when Anthropic updates available models.

## Source of Truth

The canonical model list comes from:
1. **Claude Code docs**: https://code.claude.com/docs/en/model-config
2. **Running `/model` in Claude Code** (interactive TUI — shows the live picker)
3. **CC system init event**: the `model` field shows the current default

CC does NOT expose available models programmatically (see github.com/anthropics/claude-code/issues/12612).

## Where the List Lives

**File**: `crates/cognos-engine/src/runtime/claude_code.rs`
**Constant**: `CC_MODEL_OPTIONS`

Each entry has:
- `value` — the alias CC accepts (e.g., `"sonnet"`, `"opus"`, `"haiku"`)
- `label` — display name (e.g., `"Sonnet"`)
- `description` — one-line description (e.g., `"Sonnet 4.6 · Best for everyday tasks"`)

## Update Procedure

1. **Check the current list**: search for `CC_MODEL_OPTIONS` in `claude_code.rs`
2. **Compare with CC's picker**: run `claude` interactively and type `/model`, or check the model-config docs page
3. **Update the constant**: add/remove/modify entries to match
4. **Update tests**: the `command_definitions_include_model_options` test validates the list
5. **Run tests**: `cargo test -p cognos-engine -- cc_model`
6. **Commit**: `fix: update CC model list to match current /model picker`

## Known Aliases

CC accepts these short aliases for `set_model` control requests:
- `default` — tier default
- `sonnet` — latest Sonnet
- `opus` — latest Opus
- `haiku` — latest Haiku

For 1M context variants, use the full model ID with extended context flag.
