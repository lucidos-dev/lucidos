/** The https half of the outbox hand-off (`outboxHandoff.ts`): take the
 *  export from the http origin, write it to this origin's outbox, acknowledge.
 *
 *  Nothing here sends. An unsent message lands as `unsent`, so the restore
 *  draws it as a Not sent card with Retry. Queued upload sends never reach
 *  the export, since they would send by themselves once their images land. */

import { createIndexedDbBackend } from '../store/pendingUploadBackend';
import { pendingUploadDbName, type PendingUploadBackend, type PendingUploadRecord } from '../store/pendingUploadRecords';
import { importRestoreJournal, type RestoreJournal } from '../store/restoreGenerationRecords';
import {
  createIndexedDbUnsentMessageBackend,
  unsentMessageDbName,
  type UnsentMessageBackend,
  type UnsentMessageRecord,
} from '../store/unsentMessageRecords';
import { errorDetail } from '../utils/errorDetail';
import {
  OUTBOX_HANDOFF_COMMANDS,
  OUTBOX_HANDOFF_MESSAGES,
  decodeNativeOutbox,
  handoffSourceOrigins,
  outboxRecordCount,
  parseOutboxExport,
  type OutboxExport,
  type OutboxImportAck,
} from './outboxHandoff';

/** Where imported records go: each workspace's own outbox databases. */
export interface OutboxImportTarget {
  putUnsentMessage(workspaceId: string | null, record: UnsentMessageRecord): Promise<void>;
  putPendingUpload(workspaceId: string | null, record: PendingUploadRecord): Promise<void>;
  putRestoreJournal(workspaceId: string | null, journal: RestoreJournal): Promise<void>;
}

export function indexedDbImportTarget(): OutboxImportTarget {
  const unsent = new Map<string, UnsentMessageBackend>();
  const uploads = new Map<string, PendingUploadBackend>();
  function backend<T>(cache: Map<string, T>, name: string, create: (name: string) => T): T {
    let found = cache.get(name);
    if (!found) cache.set(name, found = create(name));
    return found;
  }
  return {
    putUnsentMessage: (workspaceId, record) =>
      backend(unsent, unsentMessageDbName(workspaceId), createIndexedDbUnsentMessageBackend).put(record),
    putPendingUpload: (workspaceId, record) =>
      backend(uploads, pendingUploadDbName(workspaceId), createIndexedDbBackend).putUpload(record),
    putRestoreJournal: importRestoreJournal,
  };
}

/** Write every record, in order, and resolve with how many. Rejects on the
 *  first failed write, so the hand-off keeps the http origin's copy. */
export async function importOutbox(exported: OutboxExport, target: OutboxImportTarget): Promise<number> {
  let imported = 0;
  for (const workspace of exported.workspaces) {
    for (const record of workspace.unsentMessages) {
      await target.putUnsentMessage(workspace.workspaceId, { ...record, phase: 'unsent' });
      imported += 1;
    }
    for (const record of workspace.pendingUploads) {
      await target.putPendingUpload(workspace.workspaceId, record);
      imported += 1;
    }
    if (workspace.restoreJournal) {
      await target.putRestoreJournal(workspace.workspaceId, workspace.restoreJournal);
      imported += workspace.restoreJournal.observations.length;
    }
  }
  return imported;
}

/** Tauri `invoke`, narrowed to the native store's commands. */
export type OutboxHandoffInvoke = (
  cmd: typeof OUTBOX_HANDOFF_COMMANDS.take | typeof OUTBOX_HANDOFF_COMMANDS.acknowledge,
  args?: Record<string, unknown>,
) => Promise<unknown>;

/** The desktop window's import: take what the hand-off page stored natively,
 *  write it, then acknowledge those payloads. A crash before the
 *  acknowledgement imports the same records again at the next start, which
 *  overwrites them in place. */
export async function importNativeOutbox(invoke: OutboxHandoffInvoke, target: OutboxImportTarget): Promise<number> {
  const payloads = await invoke(OUTBOX_HANDOFF_COMMANDS.take);
  if (!Array.isArray(payloads)) throw new Error('the desktop app answered with something other than a list');
  if (payloads.length === 0) return 0;
  let imported = 0;
  for (const payload of payloads) {
    const exported = typeof payload === 'string' ? decodeNativeOutbox(payload) : null;
    if (!exported) throw new Error('this version of Lucidos cannot read the saved work');
    imported += await importOutbox(exported, target);
  }
  // By content, so a second window importing the same take cannot drop a
  // payload stored after it.
  await invoke(OUTBOX_HANDOFF_COMMANDS.acknowledge, { payloads });
  return imported;
}

/** What the import page needs from its document. */
export interface OutboxImportPage {
  /** The window that opened this one: the hand-off page, if any. */
  opener: Pick<Window, 'postMessage'> | null;
  addMessageListener(listener: (event: MessageEvent) => void): void;
  /** This page's own port, which the hand-off origin must share. */
  port: string;
  target: OutboxImportTarget;
  report(status: string): void;
  /** Leave for the picker, where a browser that must pair again pairs. */
  finish(): void;
}

function units(count: number): string {
  return count === 1 ? '1 unsent item' : `${count} unsent items`;
}

/** Answer the hand-off page that opened this one. Only that window, on a
 *  loopback http origin with this port, is heard. */
export function startOutboxImportPage(page: OutboxImportPage): void {
  const { opener } = page;
  if (!opener) {
    page.finish();
    return;
  }
  const sources = handoffSourceOrigins(page.port);
  let importing = false;
  page.addMessageListener((event) => {
    if (event.source !== opener || !sources.includes(event.origin) || importing) return;
    const reply = (ack: OutboxImportAck) => opener.postMessage(ack, event.origin);
    const exported = parseOutboxExport(event.data);
    if (!exported) {
      // An export this version cannot read is refused out loud, so the
      // hand-off page keeps its copy rather than waiting forever.
      const kind = (event.data as { kind?: unknown } | null)?.kind;
      if (kind === OUTBOX_HANDOFF_MESSAGES.export) {
        reply({ kind: OUTBOX_HANDOFF_MESSAGES.ack, error: 'this version of Lucidos cannot read the saved work' });
      }
      return;
    }
    importing = true;
    page.report(`Moving ${units(outboxRecordCount(exported))} to the secure address…`);
    importOutbox(exported, page.target).then((imported) => {
      reply({ kind: OUTBOX_HANDOFF_MESSAGES.ack, imported });
      page.finish();
    }, (err: unknown) => {
      importing = false;
      const error = errorDetail(err);
      reply({ kind: OUTBOX_HANDOFF_MESSAGES.ack, error });
      page.report(`Could not move your unsent work (${error}). It is still on the old address: press Continue there to try again.`);
    });
  });
  // The opener's origin is one of three, and a message to any other is dropped.
  for (const origin of sources) opener.postMessage({ kind: OUTBOX_HANDOFF_MESSAGES.ready }, origin);
}
