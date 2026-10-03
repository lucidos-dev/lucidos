import { signal, type Signal } from '@preact/signals';
import { fetchToolArgs, fetchToolResult, type ToolArgsPayload, type ToolResultPayload } from '../api/threads';
import { toFailed, type Loadable, type ResponseEvent } from './types';

type Step = Extract<ResponseEvent, { type: 'step' }>;

/** How many fetched steps stay cached. A result can be large, and a reader
 *  reopens the last few steps, not the whole transcript. */
const MAX_ENTRIES = 32;

/** The stripped half of a step, fetched once per event id.
 *
 *  The step detail modal draws from here, and the row prefetches on press. The
 *  fetch then usually lands before the click opens the modal. So the modal
 *  paints at its final height, rather than growing when the data arrives. An
 *  event's args and result never change, so an entry never goes stale. */
function cache<T>(fetcher: (eventId: string) => Promise<T>) {
  const entries = new Map<string, Signal<Loadable<T>>>();
  return {
    /** The entry for `eventId`, fetching it when absent or failed. A failure
     *  retries on the next request, which is the next open of the step. */
    request(eventId: string): Signal<Loadable<T>> {
      const cached = entries.get(eventId);
      if (cached && cached.value.status !== 'failed') return cached;
      const entry = cached ?? signal<Loadable<T>>({ status: 'loading' });
      entry.value = { status: 'loading' };
      entries.delete(eventId);
      entries.set(eventId, entry);
      if (entries.size > MAX_ENTRIES) entries.delete(entries.keys().next().value!);
      fetcher(eventId)
        .then((data) => { entry.value = { status: 'loaded', data }; })
        .catch((err: unknown) => { entry.value = toFailed<T>(err); });
      return entry;
    },
    clear() { entries.clear(); },
  };
}

const toolArgs = cache<ToolArgsPayload>(fetchToolArgs);
const toolResult = cache<ToolResultPayload>(fetchToolResult);

export const requestToolArgs = toolArgs.request;
export const requestToolResult = toolResult.request;

/** Start fetching whatever the step's snapshot stripped. */
export function prefetchStepDetail(step: Step): void {
  if (step.args_stripped && step.call_event_id) requestToolArgs(step.call_event_id);
  if (step.result_stripped && step.result_event_id) requestToolResult(step.result_event_id);
}

/** Drop every entry. Tests reuse event ids, and each needs a cold cache. */
export function clearStepDetailCache(): void {
  toolArgs.clear();
  toolResult.clear();
}
