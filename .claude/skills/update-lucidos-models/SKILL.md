---
name: update-lucidos-models
description: Use when the Claude Code model picker looks wrong, a new Claude, Codex or chat model ships, or the intelligence ladder needs a refresh. The picker is discovered from Claude Code itself. Covers discovery, the curated fallback file and its context windows, and the rung estimates refresh for every ladder surface.
---

# The Lucidos Claude Code model picker

The Claude Code `/model` picker in Lucidos is **discovered from Claude Code
itself** (ADR 0325), and the curated file fills what it omits (ADR 0488).
Nobody resyncs the discovered half by hand. What still needs hands is the
curated file, and the context windows it declares.

## Where the list comes from

- **Discovery** (`runtime/cc_model_discovery.rs`, probe in
  `runtime/claude_code.rs::probe_cc_models`). A cold Claude Code process, built
  like a session, gets one `initialize` control request and no prompt. Its
  reply's `models` array is the picker, in Claude Code's order, with its own
  labels and per-model effort tiers. The reply's account data is dropped.
- **Cache:** `<workspace>/.lucidos/cc-models.json`. Loaded at boot, refreshed
  in the background when missing, a day old, or when a session's handshake
  names a new Claude Code version.
- **Curated rows:** `runtime/cc_menu_options.json`. Each row with `offered`
  true (the default) follows Claude Code's own rows, unless Claude Code listed
  that value. Before the first probe succeeds they are the whole list.

The offered list is Claude Code's rows, then the offered curated rows it did not
list. A model Claude Code accepts but omits is therefore still offered. A new
Claude model reaches the picker when Claude Code lists it, usually after
`claude update`, or when you add a curated row for it. The `run_coding_agent`
tool, the `chat_submit` route and the picker all read this list.

## When the picker looks wrong

1. Read `models_provenance` on `GET /api/v1/claude-code/commands`. `source`
   says `discovered` or `fallback`; `error` holds the last failed probe.
2. Read `.lucidos/cc-models.json` for what the last probe saw.
3. Check the engine log for `[CcModels]` lines.
4. A list that follows the user's pins is correct. `ANTHROPIC_DEFAULT_SONNET_MODEL`
   and friends change what `sonnet` resolves to, and the picker says so.
5. To force a re-probe, delete the cache file and restart the engine.

## The curated file

`cc_menu_options.json` has three jobs: the picker before discovery, the models
Claude Code accepts but omits, and the **context-window overlay** that discovery
cannot supply. Keep its rows roughly current.

- Set `"offered": false` on a model that should leave the picker. The row stays
  as overlay only, so a thread already on it keeps its window, and a new spawn
  refuses it. Opus 5 and the Fable rows are overlay only.
- A curated row never carries a `resolvedModel`. An alias shows "runs X" only
  from a discovered row.

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

## Refreshing the rung estimates

`crates/lucidos-engine/src/llm/rung_estimates.json` holds the *rung
estimates* (ADR 0525). It bands every estimated (model, effort) pair, and
lists each *ladder surface*'s shipped pairs (chat, Claude Code, Codex) in
order, each with `on`. Each pair carries a `general_band` and a `coding_band`,
one per *intelligence profile*, plus token, cost and time bands. It holds no
raw figure: Artificial Analysis (AA) forbids republishing its content, and the
file ships to the public mirror. An intelligence band is a score in steps of
2 points, rounded half up (ADR 0527).

`estimate_rungs.py`, beside this file, writes it. Refresh it on every model
release, and when AA publishes a new index version.

Keep the inputs in `docs/plans/rung-estimate-inputs/`. The release drops
`docs/plans/**`, so they stay in the private repository and never ship. Its
README names each file's source.

1. **List the pairs.** Claude Code: the discovered list
   (`.lucidos/cc-models.json`) plus the curated rows. Codex: run
   `codex app-server`, send `initialize`, then `model/list`. Copy each model's
   `defaultReasoningEffort` into its row's `default_effort` in
   `runtime/codex_menu_options.json`. A model the list no longer offers keeps
   `medium`. Chat: every
   builtin registry model that is not retired, at the tiers
   `llm::reasoning::supported_efforts` gives it on any of its routes. Add each
   model to `MODELS` in the script, with its Epoch name and each surface's
   efforts.
2. **Pull AA's Intelligence Index figures into `aa_rungs.csv`.** AA has one
   page per model and effort, `https://artificialanalysis.ai/models/<slug>-<effort>`,
   and the bare slug is the top effort. Any page embeds every model's record.
   Take the score as AA displays it, the cost per task, the time per task, the
   total output tokens and the output speed. The record names them
   `intelligenceIndex`, `intelligenceIndexCostPerTask.cost.total`,
   `intelligenceIndexTimePerTask`, `canonicalIntelligenceIndexTokenCount.output`
   and `timescaleData.medianOutputSpeed`.
   - Leave a figure AA did not publish empty. A pair with no cost or no
     tokens is skipped, and stays unbanded.
   - A `(Non-reasoning)` variant is the `none` pair.
   - A variant named for no effort is the model's registry default effort.
     With no default effort, leave it out.
3. **Pull AA's Coding Agent Index into `aa_coding.csv`**, from
   `https://artificialanalysis.ai/agents/coding-agents`: each row's
   `indexScore` by its `displayLabel`. It does not cover every effort. A pair
   it lacks takes its general score plus its model's mean measured gap
   (coding minus general), flagged `coding_estimated`. A model with no
   measured pair has gap zero.
4. **Pull the other inputs.**
   - `eci_scores.csv`: `https://epoch.ai/data/eci_scores.csv` (CC BY 4.0).
   - `openrouter_speed.csv`: each model's standard-tier throughput from
     OpenRouter's endpoints API. Take the endpoint whose tag is the vendor,
     and skip the pricier `fast` ones. The call needs the workspace's
     OpenRouter key. Read it into a shell variable with `psql`, and never
     print it or put it in a command line. A model it lacks takes AA's speed.
5. **Check the prices.** The script reads each model's output price from
   the shipped *price table*, `llm/model_prices.json`, on the card in force
   today. A model with no card stops it. Add or update cards there first,
   following `system-knowhow/model-price-check.md`.
6. **Run the script.** `PYTHONDONTWRITEBYTECODE=1 python3
   .claude/skills/update-lucidos-models/estimate_rungs.py
   docs/plans/rung-estimate-inputs` rewrites the file. Add `--print` to read
   the estimates first. Its docstring states the blend and both profiles.
7. **Run the tests.** `./scripts/test-engine.sh -- -- runtime::intelligence_ladder`
   checks every pair's fields, that each surface's on pairs form a frontier,
   and that the file ships integer bands only. Its expected-ladder tests name
   the shipped rungs, so a refresh that moves them updates the tests in the
   same change.

Not covered yet:

- codex-cli 0.163.0 offers an `ultra` effort that no menu row offers, so it
  has no rung.
- AA publishes no cost for GLM 5.2 or Grok 4.20 at an effort chat offers,
  and does not list Laguna S 2.1 or Ox Alpha. It measures Muse Spark 1.2
  only at xhigh, which chat does not offer. The other free models have no
  default effort. Their chat pairs stay unbanded.
