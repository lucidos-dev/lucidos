---
name: update-lucidos-models
description: Required for every Lucidos model addition across chat, Claude Code and Codex. Covers registration, routes, prices, reasoning, menus, comparable benchmark estimates, availability-aware ladders and cost-justified defaults. Also use for picker diagnosis or estimate refreshes.
---

# Update Lucidos models

Every model addition ships registration, estimates, ladder review and default review together (ADR 0732).
Use this ordered workflow across chat, Claude Code and Codex.
A picker repair can use its conditional section; a model addition must complete every shared step.

## Required workflow

1. **Establish the contract.** Read the provider's model documentation and each affected CLI's discovery response.
   Record sources, supported ids, context windows, route-specific capabilities and documented effort defaults.
   Leave an undocumented API default effort absent; distinguish it from a CLI's model-only default.
2. **Register and route.** Update `llm/model_registry.rs` and the affected provider's wire-id and capability mappings.
   Preserve account-specific eligibility, explicit provider pins and older model selections.
3. **Price and reason.** Add dated cards in `llm/model_prices.json` from official prices, including cache and long-context tiers.
   Update route-specific reasoning support and request serialization, with protocol regressions.
4. **Update menus.** Update `runtime/codex_menu_options.json` for Codex and the conditional Claude Code section below when affected.
   Keep catalogue discovery separate from whether the workspace can run the model.
5. **Refresh comparable evidence.** Follow **Refreshing the rung estimates** below for every affected ladder surface.
   Archive source records privately, validate the cohort and regenerate bands plus dominance metadata.
   Missing coding observations use the documented estimation path and remain explicitly estimated.
   Never invent measurements or copy scores between efforts.

   Missing cost or token evidence can leave pairs unbanded; report that coverage and review their ladder behavior explicitly.
   Missing observations never waive ladder or default review.
6. **Review available frontiers.** Check all-provider and single-provider setups, account exclusions and route-specific effort support.
   The Lucidos Agent default frontier uses available pairs and their generated dominators, retaining an available default rung.
   Resolve the route the router actually chooses before inspecting its availability; never substitute around a refusal or provider pin.
   Keep catalogue entries and stored overrides independent of automatic frontier filtering.
7. **Justify current defaults.** Compare capability evidence and input, output, cache and long-context prices against each existing default.
   Prefer the current supported generation when capability and cost justify it; release recency alone never authorizes an upgrade.
   Keep a substantially costlier successor as a premium option unless the owner chooses otherwise.
   Record the comparison and rationale whether the default changes or stays.

   Audit automatic chat selection, both default implementers and generic background fallbacks.
   Keep historical evaluation rosters and usage seeds attached to the models actually measured.
   Preserve stored model/effort preferences, explicit older-model pins and recorded thread selections; never migrate them to a new default.
8. **Verify and document.** Add protocol, provider-subset, frontier and default-preservation regressions for the affected surfaces.
   Update knowhow and glossaries, run the estimator tests, then complete `/harden` over the entire change.

### Completion checklist

- Registration, routes, prices, reasoning and affected menus agree on each supported model and effort.
- Active benchmark observations have comparable cohort evidence; missing measurements remain explicit estimates or documented unbanded coverage.
- Generated bands and all dominator identities pass subset parity and privacy checks; unrelated calibration changes are deliberate.
- Available frontiers, default retention, catalogue visibility and stored overrides pass their shared-reader tests.
- Every affected default has a capability and cost rationale, including any decision to retain its predecessor.
- Fresh-workspace choices and existing preference/thread pins pass regressions without preference rewrites.
- Knowhow matches the behavior, and the finished branch has a hardened marker.

## Claude Code discovery and picker

Read this section when Claude Code is affected.

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
5. A curated row missing from the picker may be a *learned refusal*.
   `.lucidos/cc-refused-models.json` holds each model Claude Code ran another
   model in place of, with the version and account that did it (ADR 0674).
   It clears when Claude Code lists the model, a probe finds another version,
   or a session reports another live account.
6. To force a re-probe, delete the cache file and restart the engine. Delete
   the refusals file too to drop the refusals.

## The curated file

`cc_menu_options.json` has three jobs: the picker before discovery, the models
Claude Code accepts but omits, and the **context-window overlay** that discovery
cannot supply. Keep its rows roughly current.

- Set `"offered": false` on a model that should leave the picker. The row stays
  as overlay only, so a thread already on it keeps its window, and a new spawn
  refuses it. Opus 5, Fable 5 and Fable 5.1 are overlay only.
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

`crates/lucidos-core/src/llm/rung_estimates.json` holds the *rung
estimates* (ADR 0525). It bands every estimated (model, effort) pair, and
lists each *ladder surface*'s shipped pairs (chat, Claude Code, Codex) in
order, each with `on`. Each pair carries a `general_band` and a `coding_band`,
one per *intelligence profile*, plus token, cost and time bands. It also records every dominating pair identity for each surface pair.
Lucidos Agent runtime filters these identities by route availability. It holds no
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
   model to `MODELS` in the script, with its AA release name and each
   surface's efforts.
2. **Pull AA's Intelligence Index figures into `aa_rungs.csv`.** AA has one
   page per model and effort, `https://artificialanalysis.ai/models/<slug>-<effort>`,
   and the bare slug is the top effort. A page embeds only some models'
   records, so fetch every cohort model's pages.
   - **Re-pull every row from one snapshot, never only the new model.** AA
     re-versions its index, and a version change moves every score and cost.
     Rows from two versions make one model look far cheaper or costlier.
   - A model AA no longer scores has no row figures, and its pairs go
     unbanded. Keeping its old figures would mix versions again. Drop it from
     `FIT_MODELS` when it was there.
   Take the score as AA displays it, the cost per task, the time per task, the
   total output tokens and the output speed. The record names them
   `intelligenceIndex`, `intelligenceIndexCostPerTask.cost.total`,
   `intelligenceIndexTimePerTask`, `canonicalIntelligenceIndexTokenCount.output`
   and `timescaleData.medianOutputSpeed`.
   Newer pages expose first-party speed as `headline.outputSpeed` instead.
   - Leave a figure AA did not publish empty. A pair with no cost or no
     tokens is skipped, and stays unbanded.
   - A `(Non-reasoning)` variant is the `none` pair.
   - A variant named for no effort is the model's registry default effort.
     With no default effort, leave it out.
3. **Pull AA's Coding Agent Index into `aa_coding.csv`**, from
   `https://artificialanalysis.ai/agents/coding-agents`: each row's
   `indexScore` by its explicit model and effort. Retain complete source records
   in `coding-provenance.json`, using `coding_snapshot(records, source_url, extracted)`.
   It validates one evaluation cohort and preserves each harness version.
   Archive previous inputs before replacing the active snapshot.
   The generator checks CSV scores against this evidence before calibration.

   It does not cover every effort. A pair
   it lacks takes its general score plus its model's mean measured gap
   (coding minus general), flagged `coding_estimated`. A model with no
   measured pair has gap zero.
4. **Pull the other inputs.**
   - `openrouter_speed.csv`: each model's standard-tier throughput from
     OpenRouter's endpoints API. Take the endpoint whose tag is the vendor,
     and skip the pricier `fast` ones. Public endpoints can be read without a key.
     If authentication is required, read the workspace key into a shell variable
     with `psql`; never print it or put it in a command line.
     Missing or null throughput falls back to AA's speed.
5. **Check the prices.** The script reads each model's output price from
   the shipped *price table*, `llm/model_prices.json`, on the card in force
   today. A model with no card stops it. Add or update cards there first,
   following `system-knowhow/model-price-check.md`.
   - Preserve import evidence privately alongside the inputs: source URL,
     extraction date, exact variant/display label, model and explicit effort,
     harness and relevant settings. Do not infer an effort from a bare family
     name or copy one observation across efforts. A duplicate pair is an error.
     Historical rows without this evidence remain unverified; do not manufacture
     provenance retroactively.
6. **Run the script.** `PYTHONDONTWRITEBYTECODE=1 python3
   .claude/skills/update-lucidos-models/estimate_rungs.py
   docs/plans/rung-estimate-inputs` rewrites the file. Add `--print` to read
   the estimates first. Its docstring states the blend and both profiles.
   Review every normalization diagnostic against the source variant and harness.
   The operational estimates use a cumulative maximum over effort within each
   model, on both profiles, before sorting and frontier selection. This permits
   ties and never claims evidence of strict improvement. Raw observations are
   preserved. `coding_estimated` means absent index coverage; even covered
   scores can be smoothed, as the generated source description states.
7. **Run the tests.** First run `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest
   discover -s .claude/skills/update-lucidos-models -p test_estimate_rungs.py`.
   The hardening gate runs these import and normalization regressions too.
 `./scripts/test-engine.sh -- -- runtime::intelligence_ladder`
   checks every pair's fields and that each surface's on pairs form a
   frontier. The generated flags retain each shipped default.
   Lucidos Agent runtime flags use available dominators and retain its available current default.
   Test provider subsets, route refusals, default retention and stored overrides alongside the offline frontier. The tests
   also check that the file ships integer bands only. Its expected-ladder tests name
   the shipped rungs, so a refresh that moves them updates the tests in the
   same change.

Not covered yet:

- codex-cli 0.163.0 offers an `ultra` effort that no menu row offers, so it
  has no rung.
- AA's current index no longer scores GPT-6 Sol below max, GPT-5.6 Sol,
  GPT-5.6 Terra and Luna below max, Gemini 3.1 Pro or Grok 4.6. Their pairs
  stay unbanded.
- AA publishes no cost for GLM 5.2 or Grok 4.20 at an effort chat offers,
  and does not list Laguna S 2.1 or Ox Alpha. It measures Muse Spark 1.2
  only at xhigh, which chat does not offer. The other free models have no
  default effort. Their chat pairs stay unbanded.
