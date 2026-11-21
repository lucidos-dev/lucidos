/** The outbox hand-off across the http-to-https origin change (ADR 0745).
 *
 *  Storage belongs to an origin, so work queued on `http://localhost:<port>`
 *  is invisible on `https://localhost:<port>`. The hand-off page on the http
 *  origin (`public/outbox-handoff.js`) reads the outbox and posts it to the
 *  import page here, which writes it and acknowledges. Only then does the
 *  hand-off page clear the http origin. A leaf: types only from the stores. */

import type { PendingUploadRecord } from '../store/pendingUploadRecords';
import type { RestoreJournal } from '../store/restoreGenerationRecords';
import type { UnsentMessageRecord } from '../store/unsentMessageRecords';

/** Where the hand-off page opens the https origin. The gateway serves it as
 *  the public picker shell, so a browser that must pair again still reaches it. */
export const OUTBOX_IMPORT_PATH = '/~/outbox-import';

/** The `kind` of each message the two pages exchange. */
export const OUTBOX_HANDOFF_MESSAGES = {
  ready: 'lucidos-outbox-import-ready',
  export: 'lucidos-outbox-export',
  ack: 'lucidos-outbox-import-ack',
} as const;

export const OUTBOX_EXPORT_VERSION = 1;

/** One workspace's outbox, as the http origin held it. The restore journal
 *  rides along when it holds recovered drafts the user has not reviewed. */
export interface OutboxWorkspace {
  workspaceId: string | null;
  unsentMessages: UnsentMessageRecord[];
  pendingUploads: PendingUploadRecord[];
  restoreJournal?: RestoreJournal;
}

export interface OutboxExport {
  kind: typeof OUTBOX_HANDOFF_MESSAGES.export;
  version: typeof OUTBOX_EXPORT_VERSION;
  workspaces: OutboxWorkspace[];
}

export type OutboxImportAck =
  | { kind: typeof OUTBOX_HANDOFF_MESSAGES.ack; imported: number }
  | { kind: typeof OUTBOX_HANDOFF_MESSAGES.ack; error: string };

/** The http origins a hand-off may come from: loopback, on the import page's
 *  own port. The gateway holds that port, so nothing else can serve them. */
export function handoffSourceOrigins(port: string): string[] {
  return ['localhost', '127.0.0.1', '[::1]'].map((host) => `http://${host}:${port}`);
}

/** Each unsent message, pending upload and journal observation counts once. */
export function outboxRecordCount(exported: OutboxExport): number {
  return exported.workspaces.reduce((sum, w) => sum + w.unsentMessages.length + w.pendingUploads.length
    + (w.restoreJournal?.observations.length ?? 0), 0);
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isUnsentMessage(value: unknown): value is UnsentMessageRecord {
  return isObject(value) && typeof value.eventId === 'string' && typeof value.threadId === 'string'
    && isObject(value.body) && typeof value.sentAt === 'string';
}

function isPendingUpload(value: unknown): value is PendingUploadRecord {
  return isObject(value) && typeof value.localId === 'string' && typeof value.threadId === 'string'
    && value.bytes instanceof ArrayBuffer;
}

function isEntryList(value: unknown): value is Array<[IDBValidKey, unknown]> {
  return Array.isArray(value) && value.every((entry) => Array.isArray(entry) && entry.length === 2);
}

function isRestoreJournal(value: unknown): value is RestoreJournal {
  return isObject(value) && isEntryList(value.observations) && isEntryList(value.uploadBytes)
    && value.uploadBytes.every(([, payload]) => isObject(payload) && payload.bytes instanceof ArrayBuffer);
}

function isWorkspace(value: unknown): value is OutboxWorkspace {
  return isObject(value)
    && (value.workspaceId === null || typeof value.workspaceId === 'string')
    && Array.isArray(value.unsentMessages) && value.unsentMessages.every(isUnsentMessage)
    && Array.isArray(value.pendingUploads) && value.pendingUploads.every(isPendingUpload)
    && (value.restoreJournal === undefined || isRestoreJournal(value.restoreJournal));
}

/** The export a message carries, or null when it is not one this version reads. */
export function parseOutboxExport(data: unknown): OutboxExport | null {
  if (!isObject(data) || data.kind !== OUTBOX_HANDOFF_MESSAGES.export || data.version !== OUTBOX_EXPORT_VERSION) return null;
  if (!Array.isArray(data.workspaces) || !data.workspaces.every(isWorkspace)) return null;
  return data as unknown as OutboxExport;
}

/** The desktop window's native outbox store (`outbox_handoff.rs`). The window
 *  has no second window to post to, so its hand-off page stores the export
 *  there and the https page takes it. */
export const OUTBOX_HANDOFF_COMMANDS = {
  store: 'store_outbox_handoff',
  take: 'take_outbox_handoff',
  acknowledge: 'acknowledge_outbox_handoff',
} as const;

function fromBase64(text: string): ArrayBuffer {
  const binary = atob(text);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes.buffer;
}

/** Every `{ bytes }` holder in a native export: each pending upload, and each
 *  payload in a restore journal's byte store. */
function byteHolders(workspace: Record<string, unknown>): unknown[] | null {
  if (!Array.isArray(workspace.pendingUploads)) return null;
  const journal = workspace.restoreJournal;
  if (journal === undefined) return workspace.pendingUploads;
  if (!isObject(journal) || !isEntryList(journal.uploadBytes)) return null;
  return [...workspace.pendingUploads, ...journal.uploadBytes.map(([, payload]) => payload)];
}

/** An export as the native store keeps it: JSON, with each `bytes` field in
 *  base64. Null when it is not one this version reads. */
export function decodeNativeOutbox(payload: string): OutboxExport | null {
  let data: unknown;
  try {
    data = JSON.parse(payload);
  } catch {
    return null;
  }
  if (!isObject(data) || !Array.isArray(data.workspaces)) return null;
  for (const workspace of data.workspaces as unknown[]) {
    const holders = isObject(workspace) ? byteHolders(workspace) : null;
    if (!holders) return null;
    for (const holder of holders) {
      if (!isObject(holder) || typeof holder.bytes !== 'string') return null;
      try {
        holder.bytes = fromBase64(holder.bytes);
      } catch {
        return null;
      }
    }
  }
  return parseOutboxExport(data);
}
