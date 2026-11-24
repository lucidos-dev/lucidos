import { filterListItems } from '../utils/listFilter';
import { REASONING_LEVELS, availableReasoningLevels } from './reasoningLevels';


export interface ProviderChoice {
  value: string;
  label: string;
  configured: boolean;
  reasoningEfforts: readonly string[];
}

export interface ModelChoice {
  value: string;
  label: string;
  description?: string;
  reasoningEfforts: readonly string[];
  providers?: readonly ProviderChoice[];
  section?: string;
  defaultProvider?: string;
  recommendedEffort?: string;
}

export interface TierChoice {
  value: string;
  label: string;
  description?: string;
  recommended?: boolean;
}

export const EFFORT_LADDER: readonly string[] = REASONING_LEVELS.map((l) => l.value);

export function tierOptions(
  offered: readonly string[],
  vocabulary: readonly TierChoice[],
): TierChoice[] {
  return vocabulary.filter((tier) => offered.includes(tier.value));
}

function markRecommended(tiers: TierChoice[], effort: string | undefined): TierChoice[] {
  return tiers.map((tier) => (tier.value === effort ? { ...tier, recommended: true } : tier));
}

export interface ProviderRow {
  value: string;
  label: string;
  configured: boolean;
  tiers: TierChoice[];
}

export interface ModelRow {
  value: string;
  label: string;
  description?: string;
  tiers: TierChoice[];
  provider: string | null;
  providerLabel: string | null;
  providers: ProviderRow[];
  section?: string;
}

const PAIR_SEPARATOR = '|';

export function encodePair(model: string, effort: string | null): string {
  return `${model}${PAIR_SEPARATOR}${effort ?? ''}`;
}

export function decodePair(value: string): { model: string; effort: string | null } {
  const at = value.lastIndexOf(PAIR_SEPARATOR);
  if (at === -1) return { model: value, effort: null };
  const effort = value.slice(at + PAIR_SEPARATOR.length);
  return { model: value.slice(0, at), effort: effort === '' ? null : effort };
}

export function wouldBeProvider(model: ModelChoice, picked: string | null): string | null {
  const providers = model.providers ?? [];
  if (picked !== null && providers.some((p) => p.value === picked)) return picked;
  return model.defaultProvider ?? null;
}

export function showsProviderStep(model: ModelChoice, provider: string | null): boolean {
  const providers = model.providers ?? [];
  if (providers.length < 2) return false;
  const configured = providers.filter((p) => p.configured).length;
  return configured >= 2 || providers.find((p) => p.value === provider)?.configured === false;
}

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
      tiers: markRecommended(
        tierOptions(onProvider?.reasoningEfforts ?? model.reasoningEfforts, vocabulary),
        model.recommendedEffort,
      ),
      provider,
      providerLabel: onProvider?.label ?? null,
      providers: showsProviderStep(model, provider)
        ? (model.providers ?? []).map((p) => ({
            value: p.value,
            label: p.label,
            configured: p.configured,
            tiers: markRecommended(tierOptions(p.reasoningEfforts, vocabulary), model.recommendedEffort),
          }))
        : [],
      section: model.section,
    };
  });
}

export function filterModelRows(rows: readonly ModelRow[], query: string): ModelRow[] {
  return [...filterListItems(rows, query)];
}

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

export function formatPair(
  modelLabel: string,
  effortLabel: string | null,
  providerLabel?: string | null,
): string {
  return [modelLabel, effortLabel, providerLabel].filter(Boolean).join(' · ');
}

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

export function lucidosTiers(registryEfforts?: readonly string[]): string[] {
  return availableReasoningLevels(registryEfforts).map((l) => l.value);
}

export function tiersOf(
  models: readonly ModelChoice[],
  modelId: string | null,
): readonly string[] {
  if (modelId === null) return [];
  return models.find((m) => m.value === modelId)?.reasoningEfforts ?? [];
}
