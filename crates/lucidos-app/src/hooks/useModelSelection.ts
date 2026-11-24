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


export interface ModelSelectionPatch {
  model: string;
  reasoningEffort: string | null;
  provider: string | null;
}

export interface ModelSelectionInput {
  models: readonly ModelChoice[];
  vocabulary: readonly TierChoice[];
  model: string | null;
  effort: string | null;
  providerFor?: (model: string) => string | null;
  labelFor?: (model: string) => string;
  effortFor?: (model: string) => string | null;
  unsetEffortLabel?: string;
  defaultEffortLabel?: (model: string) => string;
  onChange: (patch: ModelSelectionPatch) => void;
}

export interface ModelSelection {
  rows: ModelRow[];
  value: string;
  label: string;
  model: string | null;
  effort: string | null;
  provider: string | null;
  effortFor: (model: string) => string | null;
  defaultEffortLabel?: (model: string) => string;
  pick: (encoded: string, provider?: string) => void;
}

export function useModelSelection(input: ModelSelectionInput): ModelSelection {
  const { models, vocabulary, model, effort, onChange } = input;
  const rows = modelRows(models, vocabulary, input.providerFor);
  const current = rows.find((r) => r.value === model);

  const offered = current?.tiers ?? tierOptions(tiersOf(models, model), vocabulary);
  const resolvedEffort = clampToOffered(effort, offered);
  const effortLabel = offered.find((t) => t.value === resolvedEffort)?.label
    ?? resolvedEffort
    ?? (offered.length > 0 ? (input.unsetEffortLabel ?? null) : null);

  return {
    rows,
    value: encodePair(model ?? '', resolvedEffort),
    label: formatPair(
      current?.label ?? (model ? (input.labelFor?.(model) ?? model) : ''),
      effortLabel,
      current?.providerLabel,
    ),
    model,
    effort: resolvedEffort,
    provider: current?.provider ?? null,
    effortFor: (other: string) => (other === model || !input.effortFor
      ? resolvedEffort
      : input.effortFor(other)),
    defaultEffortLabel: input.defaultEffortLabel,
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
