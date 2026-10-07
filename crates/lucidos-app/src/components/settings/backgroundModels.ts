import type { BackgroundModel } from '../../api/types';
import { lucidosModelChoices, readsImages } from '../../store/actions/models';
import type { ModelChoice } from '../../store/modelSelection';

/** The heading over a background row's recommended models. */
export const RECOMMENDED_SECTION = 'Recommended';
/** The heading over every other model the row offers. */
export const OTHER_MODELS_SECTION = 'Other models';

/** The models one background row offers: every model the chat picker offers,
 *  the row's recommended ones first under their own heading, best first.
 *
 *  The engine sends `recommended` (`GET /api/v1/models/background`), so the
 *  list and the default it resolves from are one definition. A recommended
 *  model no configured provider serves is not offered, like any other.
 *
 *  No provider step: a background task stores no backend, and the router picks
 *  the model's route, so a provider pick here would be dropped.
 *
 *  A task that sends images offers only models that read them. The stored
 *  pick stays listed either way, so the row still shows what is set. */
export function backgroundModelChoices(
  recommended: readonly string[],
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
  const picks = recommended.flatMap((id) => all.filter((choice) => choice.value === id));
  if (picks.length === 0) return all;
  const rest = all.filter((choice) => !recommended.includes(choice.value));
  return [
    ...picks.map((choice) => ({ ...choice, section: RECOMMENDED_SECTION })),
    ...rest.map((choice) => ({ ...choice, section: OTHER_MODELS_SECTION })),
  ];
}

/** What a background row says under its label about the model in force, or
 *  `null` when there is nothing to say. */
export function backgroundRowCaveat(
  resolved: BackgroundModel | null,
  error: string | null,
): string | null {
  if (error) return `Could not read the default model: ${error}`;
  if (resolved && !resolved.reachable) return 'No configured provider serves this model';
  if (resolved?.needs_vision && !resolved.vision) {
    return resolved.source === 'preference'
      ? 'This model cannot read images, so images go undescribed'
      : 'No recommended model that reads images is reachable. Pick one that does';
  }
  return null;
}
