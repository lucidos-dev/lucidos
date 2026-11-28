import { signal, type Signal } from '@preact/signals';
import { fetchToolArgs, fetchToolResult, type ToolArgsPayload, type ToolResultPayload } from '../api/threads';
import { toFailed, type Loadable, type ResponseEvent } from './types';

type Step = Extract<ResponseEvent, { type: 'step' }>;

const MAX_ENTRIES = 32;

function cache<T>(fetcher: (eventId: string) => Promise<T>) {
  const entries = new Map<string, Signal<Loadable<T>>>();
  return {
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

export function prefetchStepDetail(step: Step): void {
  if (step.args_stripped && step.call_event_id) requestToolArgs(step.call_event_id);
  if (step.result_stripped && step.result_event_id) requestToolResult(step.result_event_id);
}

export function clearStepDetailCache(): void {
  toolArgs.clear();
  toolResult.clear();
}
