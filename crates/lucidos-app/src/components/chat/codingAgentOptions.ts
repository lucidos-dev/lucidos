import type { CodingAgentCommandOption } from '../../api/client';

/** Filter reasoning options through optional backend-supplied model metadata.
 *  An absent list means universal support; a present list requires an explicit
 *  matching model, so an unknown/default model never receives a restricted
 *  value by guesswork. */
export function availableReasoningOptions(
  options: CodingAgentCommandOption[],
  model: string | null,
): CodingAgentCommandOption[] {
  return options.filter(option =>
    !option.supported_models
    || (model !== null && option.supported_models.includes(model)),
  );
}

/** Preserve a reasoning selection when the next model supports it; otherwise
 *  use the strongest compatible option (the backend orders efforts low→high).
 *  Before command metadata loads, retain the selection instead of flashing an
 *  empty label. */
export function reconcileReasoningEffort(
  effort: string | null,
  model: string | null,
  options: CodingAgentCommandOption[],
): string | null {
  if (effort === null || options.length === 0) return effort;
  const available = availableReasoningOptions(options, model);
  if (available.some(option => option.value === effort)) return effort;
  return available[available.length - 1]?.value ?? null;
}
