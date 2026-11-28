import type { ContextCapture } from '../../store/types';

export type MissingBodyReason = 'section-never-recorded' | 'capture-off-at-step' | 'capture-off';

export function missingBodyReason(
  capture: Pick<ContextCapture, 'producer' | 'sections'>,
  captureOnNow: boolean,
): MissingBodyReason {
  if (capture.producer !== 'main_llm') return 'section-never-recorded';
  if (capture.sections.some(s => s.content !== undefined)) return 'section-never-recorded';
  return captureOnNow ? 'capture-off-at-step' : 'capture-off';
}
