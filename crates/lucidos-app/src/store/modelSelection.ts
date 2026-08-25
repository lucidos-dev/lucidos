import { REASONING_LEVELS, availableReasoningLevels } from './models';

/** A *model selection* is a model id paired with a reasoning effort, resolved
 *  together for one surface. This module is where both surfaces ask what a
 *  model offers, and how to snap an effort onto it.
 *
 *  The two used to answer separately. The Lucidos Agent filtered
 *  `REASONING_LEVELS` against the registry's `reasoning_efforts`; the
 *  coding-agent menu filtered its option list against each effort's
 *  `supported_models`. Same question, two rules, and only one of them clamped.
 *  The engine now serves `reasoning_efforts` per model row on both surfaces.
 *  For the Lucidos Agent that is `llm::reasoning::supported_efforts`, and for a
 *  coding agent `runtime::claude_code::model_and_effort_options`. So one rule
 *  covers both: ask the model row what it offers. */

/** One model a picker can offer, with the tiers it accepts. */
export interface ModelChoice {
  value: string;
  label: string;
  description?: string;
  /** Ascending, a subset of {@link EFFORT_LADDER}. Empty means the model has
   *  no reasoning tiers at all, so its picker renders no effort control. */
  reasoningEfforts: readonly string[];
}

/** One reasoning tier as a surface displays it.
 *
 *  The vocabulary differs per surface. The Lucidos Agent says "Med"; Claude
 *  Code says "Medium" and adds a description. So the label travels with the
 *  option rather than being derived from the value. */
export interface TierChoice {
  value: string;
  label: string;
  description?: string;
}

/** The unified ladder, ascending. Mirrors `llm::reasoning::EFFORT_LADDER`, and
 *  the order is load-bearing: {@link clampToOffered} measures distance along
 *  it. */
export const EFFORT_LADDER: readonly string[] = REASONING_LEVELS.map((l) => l.value);

/** The tiers to OFFER for a model: this surface's vocabulary, narrowed to what
 *  the model accepts, in the vocabulary's own order. Empty means no effort
 *  control, which is how image generation renders as a model alone. */
export function tierOptions(
  offered: readonly string[],
  vocabulary: readonly TierChoice[],
): TierChoice[] {
  return vocabulary.filter((tier) => offered.includes(tier.value));
}

/** One row of the picker's MODEL step: a model, and the tiers picking it opens.
 *
 *  An empty `tiers` means there is no second step, so this row IS the whole
 *  selection. Image generation is that case. */
export interface ModelRow {
  /** The model id. Not an encoded pair: a model alone is not a selection yet. */
  value: string;
  label: string;
  description?: string;
  /** This surface's vocabulary, narrowed to what the model accepts. */
  tiers: TierChoice[];
}

/** The separator between the two halves of an encoded pair.
 *
 *  A model id already carries `-`, `.`, `@`, `[`, `]`, `:` and `/`, so the
 *  separator has to be something none of them use. The registry is
 *  user-extensible though, so nothing can promise an id will never carry it. */
const PAIR_SEPARATOR = '|';

/** Join a pair into one string a picker row can carry.
 *
 *  A tierless model still gets the separator, with nothing after it. Leaving it
 *  off would make the encoding ambiguous for an id that carries one: tierless
 *  `a|b` would read back as `a` at tier `b`, which is a DIFFERENT and possibly
 *  real row. The trailing separator keeps the two apart. */
export function encodePair(model: string, effort: string | null): string {
  return `${model}${PAIR_SEPARATOR}${effort ?? ''}`;
}

/** Split an encoded pair back into its halves.
 *
 *  Splits at the LAST separator, so the model keeps everything before it. An
 *  empty tail is a model with no tiers, which is a whole selection on its own.
 *  A value with no separator at all is not one of ours, so it is read as a bare
 *  model rather than rejected. */
export function decodePair(value: string): { model: string; effort: string | null } {
  const at = value.lastIndexOf(PAIR_SEPARATOR);
  if (at === -1) return { model: value, effort: null };
  const effort = value.slice(at + PAIR_SEPARATOR.length);
  return { model: value.slice(0, at), effort: effort === '' ? null : effort };
}

/** Every model a picker offers, in the order it was given them.
 *
 *  This is the picker's FIRST step. The second is one model's `tiers`, and only
 *  a choice there reports a pair. The flat cross product this replaced ran past
 *  160 rows on the Lucidos registry, and grouping never made that readable.
 *  See `docs/plans/2026-08-23-two-step-model-picker.md`. */
export function modelRows(
  models: readonly ModelChoice[],
  vocabulary: readonly TierChoice[],
): ModelRow[] {
  return models.map((model) => ({
    value: model.value,
    label: model.label,
    description: model.description,
    tiers: tierOptions(model.reasoningEfforts, vocabulary),
  }));
}

/** Narrow the model step to a query, matching the model's own name.
 *
 *  Every whitespace-separated term must appear, so `opus 1m` finds
 *  `Opus 5 (1M)`. Tiers are deliberately not matched: they are the next step,
 *  and every model offers much the same handful. */
export function filterModelRows(
  rows: readonly ModelRow[],
  query: string,
): ModelRow[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return [...rows];
  return rows.filter((row) => {
    const hay = row.label.toLowerCase();
    return terms.every((t) => hay.includes(t));
  });
}

/** How an encoded pair reads on its own, resolved against the model step.
 *
 *  A tier label alone does not say which model it belongs to, and a toast
 *  confirming a pick has to. Falls back to the raw halves for a model the rows
 *  no longer list, which is better than reading blank. */
export function pairLabelOf(rows: readonly ModelRow[], encoded: string): string {
  const { model, effort } = decodePair(encoded);
  const row = rows.find((r) => r.value === model);
  const tier = row?.tiers.find((t) => t.value === effort);
  return formatPair(row?.label ?? model, tier?.label ?? effort);
}

/** How a pair reads on its own: `Opus 5 (1M) · X-High`.
 *
 *  The model alone when it offers no tiers, so an image model does not trail a
 *  separator with nothing after it. */
export function formatPair(modelLabel: string, effortLabel: string | null): string {
  return effortLabel ? `${modelLabel} · ${effortLabel}` : modelLabel;
}

/** Snap `effort` onto the nearest tier in `options`, breaking ties toward the
 *  HIGHER tier. Mirrors `llm::reasoning::clamp_effort`, which enforces the same
 *  rule on the request itself; keep the two in step.
 *
 *  `null` in two cases, and they mean different things. Either the model offers
 *  nothing, so there is no effort control at all, or nothing is selected yet. A
 *  picker that answered the second case would claim a tier the request will not
 *  carry. A coding-agent menu with no session and no pick is exactly that.
 *
 *  It diverges from Rust in ONE case, deliberately: a value that is not a tier
 *  at all. Rust DROPS it and lets the provider default apply. Inventing a tier
 *  there would make a typo silently buy the most expensive reasoning the model
 *  has. Here the caller is a picker that has to show something selected, so an
 *  unrecognised value takes the top offered tier. Safe because it is a display
 *  choice: whatever this returns is re-checked at the wire. */
export function clampToOffered(
  effort: string | null,
  options: readonly TierChoice[],
): string | null {
  if (options.length === 0 || effort === null) return null;
  if (options.some((o) => o.value === effort)) return effort;
  const target = EFFORT_LADDER.indexOf(effort);
  if (target === -1) return options[options.length - 1].value;
  return options
    .map((o) => ({ value: o.value, dist: Math.abs(EFFORT_LADDER.indexOf(o.value) - target) }))
    .reduce((best, cur) => (cur.dist <= best.dist ? cur : best))
    .value;
}

/** The tiers a Lucidos Agent model offers.
 *
 *  `registryEfforts` is the engine's own answer from `GET /api/v1/models`, and
 *  `undefined` means it could not answer: the registry has not loaded, the id
 *  has no row, or the engine predates the field. Only then does the id-shape
 *  heuristic in `store/models.ts` stand in. Callers get the registry answer
 *  from `modelReasoningEfforts` in `store/actions/models.ts`, which is where
 *  the loaded registry lives. */
export function lucidosTiers(modelId: string, registryEfforts?: readonly string[]): string[] {
  return availableReasoningLevels(modelId, registryEfforts).map((l) => l.value);
}

/** The tiers one offered model accepts. The single lookup both surfaces use,
 *  and what `useModelSelection` asks on every render and every pick.
 *
 *  An id with no row offers nothing we can vouch for. An effort sent to a model
 *  that rejects it fails the whole turn, with `validate_codex_effort` as the
 *  backstop. So an unknown model renders no effort control rather than a
 *  guessed one. */
export function tiersOf(
  models: readonly ModelChoice[],
  modelId: string | null,
): readonly string[] {
  if (modelId === null) return [];
  return models.find((m) => m.value === modelId)?.reasoningEfforts ?? [];
}
