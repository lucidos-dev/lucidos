/** A leaf shared by HTTP and stream startup. Installed before client effects;
 * tests and isolated clients that do not start the shell have no gate. */
import { signal } from '@preact/signals';
import type { QueuedUploadSendRecord } from '../store/pendingUploadRecords';

/** Exact queued intent, including its page owner. A thread id alone would
 * suppress every later send on that thread. Canonicalize context object keys
 * so storage cloning and object insertion order cannot change the identity. */
export function queuedRestoreSourceId(record: QueuedUploadSendRecord): string {
  const canonical = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(canonical);
    if (value !== null && typeof value === 'object') {
      return Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)
        .map(([key, entry]) => [key, canonical(entry)]));
    }
    return value;
  };
  // Origin metadata classifies startup admission; it is not send intent. Live
  // queue captures and older retained records may omit it.
  return JSON.stringify(canonical({ threadId: record.threadId, ownerId: record.ownerId,
    text: record.text, localIds: record.localIds, intent: record.intent }));
}
/** Where the current connection's admission is: waiting for the stream's
 *  hello, then one name per step, so a stall can say which step hung. */
export type RestoreAdmissionPhase =
  | 'awaiting-hello'
  | 'generation'
  | 'local-records'
  | 'historical-sources'
  | 'journal'
  | 'recoveries'
  | 'draft-recovery'
  | 'admitted';

export interface RestoreAdmission {
  wait(): Promise<void>;
  ready(): boolean;
  phase(): RestoreAdmissionPhase;
}

let admission: RestoreAdmission | undefined;
const revision = signal(0);

export function restoreAdmissionChanged(): void { revision.value = revision.peek() + 1; }
const preservedUploads = new Set<string>();
const preservedSends = new Set<string>();
const preservedQueuedSends = new Set<string>();

export function preserveRestoreSources(uploads: string[], sends: string[], queued: string[]): void {
  for (const id of uploads) preservedUploads.add(id);
  for (const id of sends) preservedSends.add(id);
  for (const id of queued) preservedQueuedSends.add(id);
}

export function restoreSourcePreserved(kind: 'upload' | 'send' | 'queued', id: string): boolean {
  return (kind === 'upload' ? preservedUploads : kind === 'send' ? preservedSends : preservedQueuedSends).has(id);
}

export function installRestoreAdmission(next: RestoreAdmission): void {
  admission = next;
  restoreAdmissionChanged();
}

export function restoreAdmissionReady(): boolean {
  void revision.value;
  return admission?.ready() ?? true;
}

export function restoreAdmissionPhase(): RestoreAdmissionPhase {
  return admission?.phase() ?? 'admitted';
}

export async function waitForRestoreAdmission(): Promise<void> {
  await admission?.wait();
}

/** Bootstrapping identity and health cannot depend on stream admission. */
export function isRestoreBootstrapRequest(url: string): boolean {
  return /\/(health|devices\/register|internal\/client-log)(\?|$)/.test(url);
}
