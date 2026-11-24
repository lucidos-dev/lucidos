import type { ContextCapture } from '../../store/types';
import type { ContextCapturePayload } from '../../api/threads';

export function mergeContextCaptureSections(
  snap: ContextCapture,
  payload: ContextCapturePayload,
): ContextCapture {
  return {
    ...snap,
    sections: payload.sections,
    tools: payload.tools,
    sections_stripped: false,
  };
}

export function needsLazyFetch(snap: ContextCapture): boolean {
  return Boolean(snap.sections_stripped && snap.event_id);
}
