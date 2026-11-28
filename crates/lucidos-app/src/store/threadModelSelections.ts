
import { threadMap, currentModel } from './store';
import { accountChatEffort, storedChatModel } from './actions/preferences';
import type { StoredEvent } from './thread-events';
import { getThreadEventsBump } from './threadActivity';

type PairEvent = Extract<StoredEvent, { type: 'MessageReceived' | 'TriggerStarted' | 'ChatSettingsChanged' | 'ChatTurnSettingsResolved' }>;

function isPairEvent(event: StoredEvent): event is PairEvent {
  if (event.type === 'MessageReceived' && (event as { request_event_id?: string }).request_event_id) return false;
  return event.type === 'MessageReceived'
    || event.type === 'TriggerStarted'
    || event.type === 'ChatSettingsChanged'
    || event.type === 'ChatTurnSettingsResolved';
}

type PairFields = Pick<PairEvent, 'model' | 'reasoning_effort' | 'provider'>;

function newestPairValue(
  threadId: string | null | undefined,
  read: (event: PairFields) => unknown,
): string | undefined {
  if (!threadId) return undefined;
  getThreadEventsBump(threadId);
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  let bestSeq = -Infinity;
  let bestValue: string | undefined;
  const consider = (seq: number, fields: PairFields): void => {
    const value = read(fields);
    if (typeof value !== 'string' || value === '') return;
    if (seq > bestSeq) {
      bestSeq = seq;
      bestValue = value;
    }
  };
  for (const [seq, event] of thread.events) {
    if (isPairEvent(event)) consider(seq, event);
  }
  for (const stamp of thread.pairStamps ?? []) consider(stamp.sequence, stamp);
  return bestValue;
}

export function lastThreadModel(threadId: string | null | undefined): string | undefined {
  return newestPairValue(threadId, (event) => event.model);
}

function lastThreadReasoningEffort(
  threadId: string | null | undefined,
  model: string | undefined,
): string | undefined {
  return newestPairValue(threadId, (event) => ((event.model || undefined) === model ? event.reasoning_effort : undefined));
}

export function lastThreadProvider(
  threadId: string | null | undefined,
  model: string,
): string | undefined {
  return newestPairValue(threadId, (event) => (event.model === model ? event.provider : undefined));
}


export function resolveActiveThreadModel(threadId: string | null | undefined): string {
  return lastThreadModel(threadId) ?? currentModel.value;
}

export function resolveActiveThreadProvider(
  threadId: string | null | undefined,
  model: string,
): string | null {
  return lastThreadProvider(threadId, model) ?? null;
}

export function resolveActiveThreadReasoningEffort(
  threadId: string | null | undefined,
  model: string,
): string | null {
  const named = model === resolveActiveThreadModel(threadId) ? lastThreadModel(threadId) : model;
  const remembered = named === undefined && storedChatModel() !== null
    ? undefined
    : lastThreadReasoningEffort(threadId, named);
  return remembered ?? accountChatEffort(model);
}

export function chatTurnSelection(threadId: string | undefined) {
  if (threadId) getThreadEventsBump(threadId);
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  type Stamp = PairFields & { sequence: number; type: string; request_event_id?: string | null };
  const stamps: Stamp[] = [];
  for (const [sequence, event] of thread?.events ?? []) {
    if (isPairEvent(event)) stamps.push({ ...event, sequence });
  }
  for (const stamp of thread?.pairStamps ?? []) {
    stamps.push({ ...stamp, type: stamp.event_type ?? 'MessageReceived' });
  }
  const applied = stamps.filter(s => s.type === 'ChatTurnSettingsResolved')
    .sort((a, b) => b.sequence - a.sequence)[0];
  const live = applied ?? stamps.filter(s => s.type !== 'ChatSettingsChanged')
    .sort((a, b) => b.sequence - a.sequence)[0];
  const pick = stamps.filter(s => s.type === 'ChatSettingsChanged'
    && s.sequence > (live?.sequence ?? -Infinity)).sort((a, b) => b.sequence - a.sequence)[0];
  const pending = pick && live && samePair(pick, live) ? undefined : pick;
  return { live, pending };
}

function samePair(a: PairFields, b: PairFields): boolean {
  return (a.model || undefined) === (b.model || undefined)
    && (a.reasoning_effort || undefined) === (b.reasoning_effort || undefined)
    && (a.provider || undefined) === (b.provider || undefined);
}
