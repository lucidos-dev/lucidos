import { signal } from '@preact/signals';
import type { ThreadMeta } from './thread-events';

export type RowFields = Partial<Pick<ThreadMeta, 'section' | 'saved' | 'codingAgentChangeState' | 'state'>>;

export interface RowIntent {
  kind: 'archive' | 'pin' | 'unpin' | 'discard';
  fields: RowFields;
  beneath: RowFields;
  ackVersion: number | null;
}

export const rowIntents = signal<ReadonlyMap<string, RowIntent>>(new Map());

function update(change: (intents: Map<string, RowIntent>) => void): void {
  const next = new Map(rowIntents.value);
  change(next);
  rowIntents.value = next;
}

function pick(meta: ThreadMeta, fields: RowFields): RowFields {
  const picked: RowFields = {};
  for (const key of Object.keys(fields) as (keyof RowFields)[]) {
    (picked as Record<string, unknown>)[key] = meta[key];
  }
  return picked;
}

export function rowRequestInFlight(threadId: string): RowIntent['kind'] | null {
  const intent = rowIntents.value.get(threadId);
  return intent?.ackVersion === null ? intent.kind : null;
}

export function archiveRequestInFlight(threadId: string): boolean {
  return rowRequestInFlight(threadId) === 'archive';
}

export function archivePending(threadId: string): boolean {
  return rowIntents.value.get(threadId)?.kind === 'archive';
}

export function holdRowIntent(meta: ThreadMeta, kind: RowIntent['kind'], fields: RowFields): void {
  const prior = rowIntents.value.get(meta.id);
  const beneath = { ...pick(meta, fields), ...prior?.beneath };
  update((intents) => intents.set(meta.id, { kind, fields, beneath, ackVersion: null }));
  assignChanged(meta, fields);
}

export function acknowledgeRowIntent(meta: ThreadMeta, kind: RowIntent['kind'], version: number): boolean {
  const intent = rowIntents.value.get(meta.id);
  if (intent?.kind !== kind) return false;
  if (meta.summaryVersion >= version) return abandonRowIntent(meta, kind);
  update((intents) => intents.set(meta.id, { ...intent, ackVersion: version }));
  return false;
}

export function abandonRowIntent(meta: ThreadMeta, kind: RowIntent['kind']): boolean {
  const intent = rowIntents.value.get(meta.id);
  if (intent?.kind !== kind) return false;
  update((intents) => intents.delete(meta.id));
  return assignChanged(meta, intent.beneath);
}

export function liftRowIntent(meta: ThreadMeta): void {
  const intent = rowIntents.value.get(meta.id);
  if (intent) assignChanged(meta, intent.beneath);
}

export function overlayRowIntent(meta: ThreadMeta): boolean {
  const intent = rowIntents.value.get(meta.id);
  if (!intent) return false;
  const beneath = pick(meta, intent.fields);
  if (intent.ackVersion !== null && meta.summaryVersion >= intent.ackVersion) {
    update((intents) => intents.delete(meta.id));
    return false;
  }
  if (!sameFields(beneath, intent.beneath)) update((intents) => intents.set(meta.id, { ...intent, beneath }));
  return assignChanged(meta, intent.fields);
}

function sameValue(a: unknown, b: unknown): boolean {
  return a === b || JSON.stringify(a) === JSON.stringify(b);
}

function sameFields(a: RowFields, b: RowFields): boolean {
  return (Object.keys(a) as (keyof RowFields)[]).every((key) => sameValue(a[key], b[key]));
}

function assignChanged(meta: ThreadMeta, fields: RowFields): boolean {
  let changed = false;
  for (const key of Object.keys(fields) as (keyof RowFields)[]) {
    if (!sameValue(meta[key], fields[key])) {
      (meta as Record<string, unknown>)[key] = fields[key];
      changed = true;
    }
  }
  return changed;
}

export function _resetRowIntentsForTesting(): void {
  rowIntents.value = new Map();
}

export function _holdArchiveForTesting(threadIds: string[]): void {
  rowIntents.value = new Map(threadIds.map((id) => [id, { kind: 'archive', fields: {}, beneath: {}, ackVersion: null }]));
}
