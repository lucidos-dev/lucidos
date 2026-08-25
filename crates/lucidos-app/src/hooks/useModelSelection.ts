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
 *  A model selection is one thing, so picking it is ONE act, reached in two
 *  steps: a model, then one of its tiers. Only the second step reports, and it
 *  reports both halves. The hook writes no store. A compose surface hands it a
 *  per-draft writer and Settings hands it a preference writer, and neither
 *  leaks into the other.
 *
 *  `ModelSelectionPicker` is the one component that renders this. Every
 *  surface mounts it: both prompt-bar control menus, the Settings field and the
 *  trigger form. */

/** What a pick reports back. Both halves always, because the pair is picked
 *  whole. `reasoningEffort: null` means the model has no tiers and the stored
 *  effort no longer applies. */
export interface ModelSelectionPatch {
  model: string;
  reasoningEffort: string | null;
}

export interface ModelSelectionInput {
  /** Every model this picker offers, each carrying its tiers. */
  models: readonly ModelChoice[];
  /** This surface's tier vocabulary, ascending. Narrowed per model. */
  vocabulary: readonly TierChoice[];
  /** The currently selected pair. */
  model: string | null;
  effort: string | null;
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
  /** Take one encoded pair. Reports both halves, so nothing can be
   *  half-applied. */
  pick: (encoded: string) => void;
}

export function useModelSelection(input: ModelSelectionInput): ModelSelection {
  const { models, vocabulary, model, effort, onChange } = input;

  const offered = tierOptions(tiersOf(models, model), vocabulary);
  // The STORED pair can still be stale: the model may have been changed
  // elsewhere, or its tier set narrowed under it. A pick cannot leave one
  // behind any more, but a preference written before this could.
  const resolvedEffort = clampToOffered(effort, offered);

  return {
    rows: modelRows(models, vocabulary),
    value: encodePair(model ?? '', resolvedEffort),
    label: formatPair(
      models.find((m) => m.value === model)?.label ?? model ?? '',
      offered.find((t) => t.value === resolvedEffort)?.label ?? resolvedEffort,
    ),
    model,
    effort: resolvedEffort,
    pick: (encoded: string) => {
      const picked = decodePair(encoded);
      onChange({ model: picked.model, reasoningEffort: picked.effort });
    },
  };
}
