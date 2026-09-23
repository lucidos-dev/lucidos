import { REASONING_LEVELS, availableReasoningLevels } from './models';

/** A *model selection* is a model id paired with a reasoning effort, resolved
 *  together for one surface. On the Lucidos Agent's surfaces it carries a
 *  backend too. This module is where both surfaces ask what a
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

/** One backend a model can run on, as a host describes it. */
export interface ProviderChoice {
  value: string;
  label: string;
  /** Whether the workspace can reach it. An unconfigured one is still offered,
   *  badged, so a turn refused for it stays fixable from the picker. */
  configured: boolean;
  /** The efforts THIS backend accepts for the model, ascending. */
  reasoningEfforts: readonly string[];
}

/** One model a picker can offer, with the tiers it accepts. */
export interface ModelChoice {
  value: string;
  label: string;
  description?: string;
  /** Ascending, a subset of {@link EFFORT_LADDER}. Empty means the model has
   *  no reasoning tiers at all, so its picker renders no effort control. */
  reasoningEfforts: readonly string[];
  /** Every backend that can serve the model, in route order. Absent on a
   *  surface with no provider dimension, which then never shows the step. */
  providers?: readonly ProviderChoice[];
  /** The backend the model resolves to when nothing here picks one. */
  defaultProvider?: string;
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

/** One row of the picker's PROVIDER step, carrying the tiers it accepts. */
export interface ProviderRow {
  value: string;
  label: string;
  configured: boolean;
  tiers: TierChoice[];
}

/** One row of the picker's MODEL step: a model, and the steps picking it opens.
 *
 *  An empty `tiers` and no `providers` means there is no second step, so this
 *  row IS the whole selection. Image generation is that case. */
export interface ModelRow {
  /** The model id. Not an encoded pair: a model alone is not a selection yet. */
  value: string;
  label: string;
  description?: string;
  /** This surface's vocabulary, narrowed to what the would-be backend accepts. */
  tiers: TierChoice[];
  /** The backend this row runs on unless the provider step moves it. */
  provider: string | null;
  /** How the UI names that backend, or `null` on a surface with no backends.
   *  Named whether or not there is a choice, so the user sees where a turn
   *  goes. */
  providerLabel: string | null;
  /** The PROVIDER step's rows, or empty when the row has no real choice. */
  providers: ProviderRow[];
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

/** The backend a model runs on: the pick, when it names one of the model's
 *  backends, else the model's own default. A pick for another model is stale. */
export function wouldBeProvider(model: ModelChoice, picked: string | null): string | null {
  const providers = model.providers ?? [];
  if (picked !== null && providers.some((p) => p.value === picked)) return picked;
  return model.defaultProvider ?? null;
}

/** Whether a model gets a provider step: when two of its backends are
 *  configured, or when the one it would run on is not. The second keeps a
 *  refused turn fixable. One configured backend is no choice, so a workspace
 *  with only an Anthropic key picks Opus in two steps, as before. */
export function showsProviderStep(model: ModelChoice, provider: string | null): boolean {
  const providers = model.providers ?? [];
  if (providers.length < 2) return false;
  const configured = providers.filter((p) => p.configured).length;
  return configured >= 2 || providers.find((p) => p.value === provider)?.configured === false;
}

/** Every model a picker offers, in the order it was given them.
 *
 *  This is the picker's FIRST step. The second is one model's `tiers`, and the
 *  third its backends when there is a real choice. Only the last step a row
 *  opens reports. The flat cross product this replaced ran past 160 rows on
 *  the Lucidos registry. See `docs/plans/2026-08-23-two-step-model-picker.md`.
 *
 *  `pickedFor` names the backend this surface already picked for a model, so
 *  every row shows the backend a pick of it would actually reach. A thread
 *  remembers a backend per model, not only for the model in force. */
export function modelRows(
  models: readonly ModelChoice[],
  vocabulary: readonly TierChoice[],
  pickedFor?: (model: string) => string | null,
): ModelRow[] {
  return models.map((model) => {
    const provider = wouldBeProvider(model, pickedFor?.(model.value) ?? null);
    const onProvider = model.providers?.find((p) => p.value === provider);
    return {
      value: model.value,
      label: model.label,
      description: [model.description, onProvider?.label].filter(Boolean).join(' · ') || undefined,
      tiers: tierOptions(onProvider?.reasoningEfforts ?? model.reasoningEfforts, vocabulary),
      provider,
      providerLabel: onProvider?.label ?? null,
      providers: showsProviderStep(model, provider)
        ? (model.providers ?? []).map((p) => ({
            value: p.value,
            label: p.label,
            configured: p.configured,
            tiers: tierOptions(p.reasoningEfforts, vocabulary),
          }))
        : [],
    };
  });
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
export function pairLabelOf(
  rows: readonly ModelRow[],
  encoded: string,
  provider?: string | null,
): string {
  const { model, effort } = decodePair(encoded);
  const row = rows.find((r) => r.value === model);
  const onProvider = row?.providers.find((p) => p.value === provider);
  const tier = (onProvider?.tiers ?? row?.tiers)?.find((t) => t.value === effort);
  return formatPair(
    row?.label ?? model,
    tier?.label ?? effort,
    onProvider?.label ?? row?.providerLabel,
  );
}

/** How a pair reads on its own: `Opus 5 (1M) · X-High`.
 *
 *  The model alone when it offers no tiers, so an image model does not trail a
 *  separator with nothing after it. A backend is named wherever the surface
 *  knows one, choice or not: `Opus 5.5 · High · Vertex`. */
export function formatPair(
  modelLabel: string,
  effortLabel: string | null,
  providerLabel?: string | null,
): string {
  return [modelLabel, effortLabel, providerLabel].filter(Boolean).join(' · ');
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
