import { signal } from '@preact/signals';
import type { ThreadMeta } from './thread-events';

/** The row fields an optimistic archive, pin, unpin or discard sets. */
export type RowFields = Partial<Pick<ThreadMeta, 'section' | 'saved' | 'codingAgentChangeState' | 'state'>>;

/** What the user did to a thread row that the server has not yet confirmed,
 *  a *pending intent* (ADR 0437).
 *
 *  `fields` are laid over the row after every server merge until the row's
 *  `summaryVersion` reaches `ackVersion`, the version the write returned.
 *  `beneath` holds the server's own values for those fields, refreshed by each
 *  merge, so dropping the intent shows the latest server state. */
export interface RowIntent {
  kind: 'archive' | 'pin' | 'unpin' | 'discard';
  fields: RowFields;
  beneath: RowFields;
  /** Null while the write is in flight. */
  ackVersion: number | null;
}

/** Every pending row intent, by thread id. Only this module writes it. */
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

/** The row write on `threadId` still waiting for its response, if any. Two
 *  writes on one row out together could commit in either order, so a second
 *  waits until this one has answered. */
export function rowRequestInFlight(threadId: string): RowIntent['kind'] | null {
  const intent = rowIntents.value.get(threadId);
  return intent?.ackVersion === null ? intent.kind : null;
}

/** Whether the archive of `threadId` is still waiting for its response. */
export function archiveRequestInFlight(threadId: string): boolean {
  return rowRequestInFlight(threadId) === 'archive';
}

/** Whether an archive of `threadId` is pending, in flight or acknowledged. */
export function archivePending(threadId: string): boolean {
  return rowIntents.value.get(threadId)?.kind === 'archive';
}

/** Record the user's change and lay it over `meta` at once. A later intent on
 *  the same row replaces an earlier one, keeping the server values beneath. */
export function holdRowIntent(meta: ThreadMeta, kind: RowIntent['kind'], fields: RowFields): void {
  const prior = rowIntents.value.get(meta.id);
  const beneath = { ...pick(meta, fields), ...prior?.beneath };
  update((intents) => intents.set(meta.id, { kind, fields, beneath, ackVersion: null }));
  assignChanged(meta, fields);
}

/** The write landed at `version`. The intent ends at once if the row already
 *  reached it, since a frame beat the response. Returns whether `meta`
 *  changed. */
export function acknowledgeRowIntent(meta: ThreadMeta, kind: RowIntent['kind'], version: number): boolean {
  const intent = rowIntents.value.get(meta.id);
  if (intent?.kind !== kind) return false;
  if (meta.summaryVersion >= version) return abandonRowIntent(meta, kind);
  update((intents) => intents.set(meta.id, { ...intent, ackVersion: version }));
  return false;
}

/** End the intent and show the server's values again: the write failed, left
 *  this row alone, or the row already reached its version. Returns whether
 *  `meta` changed. */
export function abandonRowIntent(meta: ThreadMeta, kind: RowIntent['kind']): boolean {
  const intent = rowIntents.value.get(meta.id);
  if (intent?.kind !== kind) return false;
  update((intents) => intents.delete(meta.id));
  return assignChanged(meta, intent.beneath);
}

/** Run before a server merge into `meta`: puts the server's own values back,
 *  so a merge that leaves a field alone leaves the server's value there. */
export function liftRowIntent(meta: ThreadMeta): void {
  const intent = rowIntents.value.get(meta.id);
  if (intent) assignChanged(meta, intent.beneath);
}

/** Run after every server merge into `meta`, which `liftRowIntent` preceded.
 *  Records the merged values as the ones beneath, then either ends the intent
 *  (the row reached its version) or lays it over them again. Returns whether
 *  `meta` changed. */
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

/** Compares by value: the change state is an object the server sends fresh. */
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

/** Put these threads' archives in flight, with nothing laid over the rows. */
export function _holdArchiveForTesting(threadIds: string[]): void {
  rowIntents.value = new Map(threadIds.map((id) => [id, { kind: 'archive', fields: {}, beneath: {}, ackVersion: null }]));
}
