import {
  clampToOffered,
  decodePair,
  encodePair,
  modelRows,
  formatPair,
  tierOptions,
  tiersOf,
  type ModelChoice,
  type ModelRow,
  type TierChoice,
} from '../store/modelSelection';

/** The one place a *model selection* is resolved for a picker.
 *
 *  A model selection is one thing, so picking it is ONE act, reached in steps:
 *  a model, one of its tiers, then one of its backends when there is a real
 *  choice. Only the last step reports, and it reports every part. The hook
 *  writes no store. A compose surface hands it a per-draft writer and Settings
 *  a preference writer, and neither leaks into the other.
 *
 *  `ModelSelectionPicker` is the one component that renders this. Every
 *  surface mounts it: both prompt-bar control menus, the Settings field and the
 *  trigger form. */

/** What a pick reports back. Every part always, because the selection is
 *  picked whole. `reasoningEffort: null` means the model has no tiers and the
 *  stored effort no longer applies. `provider: null` means the model offered no
 *  choice of backend, so a stored pick no longer applies either. */
export interface ModelSelectionPatch {
  model: string;
  reasoningEffort: string | null;
  provider: string | null;
}

export interface ModelSelectionInput {
  /** Every model this picker offers, each carrying its tiers. */
  models: readonly ModelChoice[];
  /** This surface's tier vocabulary, ascending. Narrowed per model. */
  vocabulary: readonly TierChoice[];
  /** The currently selected pair. */
  model: string | null;
  effort: string | null;
  /** The backend this surface picked for a model, or `null` for the model's
   *  own default. Ignored where the models carry no providers. */
  providerFor?: (model: string) => string | null;
  onChange: (patch: ModelSelectionPatch) => void;
}

export interface ModelSelection {
  /** The MODEL step: one row per model, each carrying the tiers it opens. */
  rows: ModelRow[];
  /** The encoded pair currently in force, for matching against a tier row. */
  value: string;
  /** The pair as one string: `Opus 5 (1M) · X-High`. */
  label: string;
  model: string | null;
  /** The effort actually in force: the stored one when the model offers it,
   *  else the clamp. `null` when the model has no tiers. */
  effort: string | null;
  /** The backend in force for `model`, or `null` when it has no routes here. */
  provider: string | null;
  /** Take one encoded pair, and the backend when the provider step chose one.
   *  Reports every part, so nothing can be half-applied. */
  pick: (encoded: string, provider?: string) => void;
}

export function useModelSelection(input: ModelSelectionInput): ModelSelection {
  const { models, vocabulary, model, effort, onChange } = input;
  const rows = modelRows(models, vocabulary, input.providerFor);
  const current = rows.find((r) => r.value === model);

  // The tiers of the backend in force, since efforts differ per backend. A
  // model with no row offers nothing we can vouch for.
  const offered = current?.tiers ?? tierOptions(tiersOf(models, model), vocabulary);
  // The STORED pair can still be stale: the model may have been changed
  // elsewhere, or its tier set narrowed under it. A pick cannot leave one
  // behind any more, but a preference written before this could.
  const resolvedEffort = clampToOffered(effort, offered);

  return {
    rows,
    value: encodePair(model ?? '', resolvedEffort),
    label: formatPair(
      current?.label ?? model ?? '',
      offered.find((t) => t.value === resolvedEffort)?.label ?? resolvedEffort,
      current?.providerLabel,
    ),
    model,
    effort: resolvedEffort,
    provider: current?.provider ?? null,
    pick: (encoded: string, provider?: string) => {
      const picked = decodePair(encoded);
      onChange({
        model: picked.model,
        reasoningEffort: picked.effort,
        provider: provider ?? null,
      });
    },
  };
}
