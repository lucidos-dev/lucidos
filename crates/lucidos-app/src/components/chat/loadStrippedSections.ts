import type { ContextCapture } from '../../store/types';
import type { ContextCapturePayload } from '../../api/threads';

/** Merge the lazy-fetched `sections` + `tools` back onto a stripped
 *  `ContextCapture`. Clears `sections_stripped` so callers can tell it's now
 *  fully populated. Pure — no fetch here, the caller owns I/O. */
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

/** Whether the modal needs to lazy-fetch sections from
 *  `GET /events/:event_id/context` before rendering the section list.
 *  False on live-SSE captures (server didn't strip), false on legacy
 *  syntheses (no source `event_id` to fetch). */
export function needsLazyFetch(snap: ContextCapture): boolean {
  return Boolean(snap.sections_stripped && snap.event_id);
}
