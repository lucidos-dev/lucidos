
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

export type OutboxHandoffInvoke = (
  cmd: typeof OUTBOX_HANDOFF_COMMANDS.take | typeof OUTBOX_HANDOFF_COMMANDS.acknowledge,
  args?: Record<string, unknown>,
) => Promise<unknown>;

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
  await invoke(OUTBOX_HANDOFF_COMMANDS.acknowledge, { payloads });
  return imported;
}

export interface OutboxImportPage {
  opener: Pick<Window, 'postMessage'> | null;
  addMessageListener(listener: (event: MessageEvent) => void): void;
  port: string;
  target: OutboxImportTarget;
  report(status: string): void;
  finish(): void;
}

function units(count: number): string {
  return count === 1 ? '1 unsent item' : `${count} unsent items`;
}

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
  for (const origin of sources) opener.postMessage({ kind: OUTBOX_HANDOFF_MESSAGES.ready }, origin);
}
