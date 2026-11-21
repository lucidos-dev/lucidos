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
  /** True until this document's first admission. Absent means never. */
  provisional?(): boolean;
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

/** True until this document's first admission. A reconnect never reopens it. */
export function restoreAdmissionProvisional(): boolean {
  return admission?.provisional?.() ?? false;
}

export async function waitForRestoreAdmission(): Promise<void> {
  await admission?.wait();
}

/** Bootstrapping identity and health cannot depend on stream admission. */
export function isRestoreBootstrapRequest(url: string): boolean {
  return /\/(health|devices\/register|internal\/client-log)(\?|$)/.test(url);
}

const PROVISIONAL_READ = /\/api\/v1\/threads\/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}(\/events)?(\?|$)/i;
let provisionalReadApplied = false;

/** A *provisional read*: one thread's summary or events, on a document never
 *  admitted. It holds no old-generation state a read could overwrite, so a
 *  notification tap can show its thread while device storage hangs. Why only
 *  these two: ADR 0704. */
export function isProvisionalRead(url: string, method: string): boolean {
  return method === 'GET' && PROVISIONAL_READ.test(url) && restoreAdmissionProvisional();
}

export function noteProvisionalRead(): void { provisionalReadApplied = true; }

/** Whether a provisional read applied since the last call. Stream frames are
 *  dropped until admission, so admission must then reconcile the gap. */
export function consumeProvisionalReads(): boolean {
  const applied = provisionalReadApplied;
  provisionalReadApplied = false;
  return applied;
}
