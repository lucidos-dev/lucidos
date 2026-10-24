import type { ContextCapture } from '../../store/types';

/** Why a context section shows no body.
 *
 *  - `section-never-recorded`: the engine records no body for this section at
 *    all (tool schemas, tail blocks, every auxiliary call), so the Capture
 *    context switch would not help.
 *  - `capture-off-at-step`: the switch was off when this step ran and is on now.
 *  - `capture-off`: the switch is off, and turning it on records bodies on
 *    later steps. */
export type MissingBodyReason = 'section-never-recorded' | 'capture-off-at-step' | 'capture-off';

/** The capture carries no flag for the switch, so its bodies stand in for it.
 *  One body anywhere means the switch was on, and a missing one is by design. */
export function missingBodyReason(
  capture: Pick<ContextCapture, 'producer' | 'sections'>,
  captureOnNow: boolean,
): MissingBodyReason {
  if (capture.producer !== 'main_llm') return 'section-never-recorded';
  if (capture.sections.some(s => s.content !== undefined)) return 'section-never-recorded';
  return captureOnNow ? 'capture-off-at-step' : 'capture-off';
}
