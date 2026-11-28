import type { BackgroundModel, GenerationStep, RecommendedSelection } from '../../api/types';
import { lucidosModelChoices, readsImages } from '../../store/actions/models';
import type { ModelChoice } from '../../store/modelSelection';

export const RECOMMENDED_SECTION = 'Recommended';
export const OTHER_MODELS_SECTION = 'Other models';

export function backgroundModelChoices(
  recommended: readonly RecommendedSelection[],
  current: string | null,
  needsVision = false,
): ModelChoice[] {
  const offered = (id: string) => !needsVision || id === current || readsImages(id);
  const all = lucidosModelChoices(current)
    .filter((choice) => offered(choice.value))
    .map((choice) => ({
      ...choice,
      providers: undefined,
      defaultProvider: undefined,
    }));
  const picks = recommended.flatMap(({ model, effort }) => all
    .filter((choice) => choice.value === model)
    .map((choice) => ({ ...choice, recommendedEffort: effort ?? undefined })));
  if (picks.length === 0) return all;
  const rest = all.filter((choice) => !recommended.some((r) => r.model === choice.value));
  return [
    ...picks.map((choice) => ({ ...choice, section: RECOMMENDED_SECTION })),
    ...rest.map((choice) => ({ ...choice, section: OTHER_MODELS_SECTION })),
  ];
}

export function backgroundRowCaveat(
  resolved: BackgroundModel | null,
  error: string | null,
): string | null {
  if (error) return `Could not read the default model: ${error}`;
  if (resolved?.unresolved) return resolved.unresolved;
  if (resolved?.asks_no_model) {
    return 'No measured model is reachable, so every report wakes its parent';
  }
  if (resolved && !resolved.reachable) {
    return resolved.not_served.includes(resolved.model)
      ? 'Its provider no longer serves this model. Pick another'
      : 'No configured provider serves this model';
  }
  if (resolved?.needs_vision && !resolved.vision) {
    return resolved.source === 'preference'
      ? 'This model cannot read images, so images go undescribed'
      : 'No recommended model that reads images is reachable. Pick one that does';
  }
  const caveats = [
    resolved?.generation ? generationCaveat(resolved.model, resolved.generation) : null,
    movedPastCaveat(resolved),
  ].filter((caveat) => caveat !== null);
  return caveats.length > 0 ? caveats.join('. ') : null;
}

function movedPastCaveat(resolved: BackgroundModel | null): string | null {
  const passed = (resolved?.not_served ?? []).filter(
    (model) => !(resolved?.generation?.kind === 'predecessor' && model === resolved.model),
  );
  return passed.length > 0
    ? `Moved past ${passed.join(', ')}: its provider no longer serves it`
    : null;
}

function generationCaveat(named: string, step: GenerationStep): string {
  switch (step.kind) {
    case 'successor':
      return `Retired, so this runs its successor ${step.model}`;
    case 'predecessor':
      return `${named} is not served here, so this runs ${step.model}, an earlier version`;
    case 'held':
      return `Retired, but ${step.successor} is not served here, so this keeps running ${named}`;
  }
}
