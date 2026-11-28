# 0500: Coding agents pick an implementer from an intelligence ladder

- **Status**: Accepted
- **Amends**: 0486, 0497, 0499, 0390
- **Date**: 2026-10-17

## Context

Model and effort guidance for coding agents lived in prose. Five places named
Sonnet, Opus and the effort tiers: `IMPLEMENTER_CHOICE`, the
`implementation-plan` skill, two knowhow files and the `run_coding_agent` tool
schema. Every new model meant editing all five, and the advice drifted from
the price list it rested on.

Two measurements now exist. Artificial Analysis (AA) publishes an Intelligence
Index score, a cost per task and a time per task for every (model, effort)
pair. Our own replay of five settled bug fixes found Opus 5.5 at `medium`
equal to `high` at 0.71x the cost. AA's ratio for the same pair is 0.74x.

## Decision

Each coding-agent backend has an *intelligence ladder*: an ordered list of
*rungs*, each one model at one effort, from least to most intelligent.

- **The menu files store the shipped ladder.** `cc_menu_options.json` and
  `codex_menu_options.json` each carry an `intelligence_ladder` block. It
  lists every estimated pair, least to most intelligent. Each pair has a cost
  band, a time band and an `on` flag, and nothing else.
- **No third-party figure ships.** AA's terms forbid republishing its
  content, and the menu files ship to the public mirror. A refresh script in
  the `update-lucidos-cc-models` skill reads AA and the other inputs from the
  private `docs/plans/rung-estimate-inputs/` folder, and writes only the block.
- **The estimate weights AA at 75%.** Score blends AA with the Epoch
  Capabilities Index. Cost and time blend AA with a formula over list prices,
  output tokens and OpenRouter speed. A pair AA did not time takes the formula
  alone.
- **The shipped ladder is the ladder frontier**, computed offline: the pairs
  no other pair beats on both estimated score and cost carry `on`. The engine
  serves the stored `on` rungs the backend still offers.
- **The owner may override it** per backend, with a full ordered list
  (`intelligence_ladder_claude_code`, `intelligence_ladder_codex`). Pairs the
  backend no longer offers drop out. Nothing joins a custom list by itself.
- **The ladder binds agents, not the owner.** An agent's explicit pick off the
  effective ladder is refused with reason `off-intelligence-ladder`. The
  owner's picker, and a successor inheriting its own thread's pair, are never
  refused.
- **Lucidos owns the default implementer.** `model_claude_code` and
  `reasoning_claude_code` pin Opus 5.5 at `medium` on every Claude Code spawn
  with no pick. `model_codex` and `reasoning_codex` pin GPT-6.1 Sol at
  `medium` on every Codex spawn with no pick, so `~/.codex/config.toml` never
  decides a Lucidos thread's model.
- **A model-only pick runs at the model's default effort.** Claude Code reads
  it from the model registry, never from its settings file. Codex reads
  `default_effort` from `codex_menu_options.json`, which a `model/list` probe
  of the Codex app server fills.

A band is a multiple of Opus 5.5 at `medium`, rounded to a step of the cube
root of 2. Cost and time show as multiples of a reference rung: the backend's
default rung. The engine divides one
band by the other, so the default rung reads 1×.

Some models bill a whole call higher once its prompt passes 100k tokens,
Haiku 5.5 among them. The menu file records that price factor. The engine
weighs it by the share of calls over the line: the owner's
`long_prompt_share`, else the share measured from the last 30 days of Claude
Code calls.

### Amendments

- **0486:** the planning agent still judges the implementer. The model and
  effort names move from prose to the ladder, and the prose keeps only the
  principle.
- **0497:** a cheaper pick means a lower rung.
- **0499:** the raise means a higher rung, not a fixed `high`.
- **0390:** coding agents now get a default pair, and a model-only pick gets
  the model's default effort.
- **Codex default:** the Codex pair ships set, not unset (see Decision).
  Lucidos does not read an effort back from Codex, which echoes only an
  effort the config set.
- **0537:** the token and cost bands count steps of the sixth root of 2. The
  time band keeps the cube root.

## Rationale

A list of rungs is data, so a new model is a data edit and a test, not five
prose edits. The frontier rule needs no taste: a dominated rung is never the
right pick, since another rung is smarter and cheaper.

The frontier moves offline because the engine no longer holds the scores it
needs. Bands are coarse enough to say "about a quarter of the cost" without
copying anyone's table. The rule still has one definition, in the refresh
script.

The long-prompt share matters because a real session is not an AA task. In
this repository most Claude Code calls pass 100k tokens by the third call.
There a base-tier band understates Haiku's cost about fourfold.

The binding is on agents because agents drift toward what they know. The owner
picks by hand and may know better than AA, so the picker stays open.

Pinning a default pair makes a Lucidos thread's cost independent of a
settings file Lucidos never writes (`engine/cc_settings.rs`).

## Consequences

- An AA task is one weighted item from ten evaluations, not a coding session.
  Ratios carry over, as the 0.71x and 0.74x agreement shows. Absolute dollars
  do not.
- Haiku 5.5 joins the Claude Code ladder on estimates alone. It has never run
  as a coding agent here.
- When a model is withdrawn, the rung it beat does not return to the ladder.
  The next refresh recomputes the frontier.
- Two rungs can share a band, as GPT-5.6 Luna and GPT-6 Luna at `medium` do.
- A release refreshes the block through the `update-lucidos-cc-models` skill.
- A workspace may override the ladder from its own measurements. Lucidos does
  not measure in product.

## Alternatives considered

- **Keep the prose and fix the drift by review.** It failed already: one
  knowhow file carried a wrong Sonnet price.
- **Ship AA's figures and compute the frontier in the engine.** This was the
  first build. It republished AA's content, which its terms forbid.
- **Ship a blended score.** A score derived mostly from AA's is still their
  figure under another name. Order and bands carry what a pick needs.
- **One ladder across backends.** Switching backend changes the transcript,
  the tools and the sandbox. It stays a separate, explicit choice.
- **Bind the owner's picker too.** It would block a deliberate choice the owner
  may base on their own measurements.
- **Write Claude Code's settings file.** Lucidos never writes it, and the
  pinned default makes it irrelevant to Lucidos threads.
