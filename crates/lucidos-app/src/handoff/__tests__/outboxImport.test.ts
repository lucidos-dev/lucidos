import { describe, expect, it, vi } from 'vitest';
import type { PendingUploadRecord } from '../../store/pendingUploadRecords';
import type { RestoreJournal } from '../../store/restoreGenerationRecords';
import type { UnsentMessageRecord } from '../../store/unsentMessageRecords';
import { OUTBOX_HANDOFF_COMMANDS, OUTBOX_HANDOFF_MESSAGES } from '../outboxHandoff';
import { importNativeOutbox, importOutbox, startOutboxImportPage, type OutboxImportPage, type OutboxImportTarget } from '../outboxImport';
import { exportOf, unsentRecord, uploadRecord } from './fixtures';

function memoryTarget(fail = false) {
  const unsent: [string | null, UnsentMessageRecord][] = [];
  const uploads: [string | null, PendingUploadRecord][] = [];
  const journals: [string | null, RestoreJournal][] = [];
  const target: OutboxImportTarget = {
    putUnsentMessage: async (workspaceId, record) => {
      if (fail) throw new Error('quota exceeded');
      unsent.push([workspaceId, record]);
    },
    putPendingUpload: async (workspaceId, record) => {
      uploads.push([workspaceId, record]);
    },
    putRestoreJournal: async (workspaceId, journal) => {
      journals.push([workspaceId, journal]);
    },
  };
  return { target, unsent, uploads, journals };
}

const PORT = '5252';
const SOURCE = `http://localhost:${PORT}`;

function page(target: OutboxImportTarget, withOpener = true) {
  const opener = { postMessage: vi.fn() };
  let listener: ((event: MessageEvent) => void) | undefined;
  const statuses: string[] = [];
  const finish = vi.fn();
  const env: OutboxImportPage = {
    opener: withOpener ? opener : null,
    addMessageListener: (l) => { listener = l; },
    port: PORT,
    target,
    report: (status) => statuses.push(status),
    finish,
  };
  startOutboxImportPage(env);
  const deliver = (data: unknown, origin = SOURCE, source: unknown = opener) =>
    listener!({ data, origin, source } as unknown as MessageEvent);
  return { opener, deliver, statuses, finish };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('outbox import', () => {
  it('lands every unsent message as unsent, so nothing sends without a tap', async () => {
    const { target, unsent, uploads } = memoryTarget();
    const imported = await importOutbox(exportOf([
      { workspaceId: 'dev', unsentMessages: [unsentRecord('e1')], pendingUploads: [uploadRecord('u1')] },
      { workspaceId: null, unsentMessages: [unsentRecord('e2')], pendingUploads: [] },
    ]), target);
    expect(imported).toBe(3);
    expect(unsent.map(([ws, r]) => [ws, r.eventId, r.phase])).toEqual([['dev', 'e1', 'unsent'], [null, 'e2', 'unsent']]);
    expect(uploads.map(([ws, r]) => [ws, r.localId])).toEqual([['dev', 'u1']]);
  });

  it('writes a restore journal as carried, counting each observation', async () => {
    const { target, journals } = memoryTarget();
    const restoreJournal: RestoreJournal = {
      observations: [['workspace', { generation: 3, recoveries: [] }]],
      uploadBytes: [[['f', 0], { bytes: new ArrayBuffer(1) }]],
    };
    const imported = await importOutbox(exportOf([
      { workspaceId: 'dev', unsentMessages: [], pendingUploads: [], restoreJournal },
    ]), target);
    expect(imported).toBe(1);
    expect(journals).toEqual([['dev', restoreJournal]]);
  });

  it('greets every loopback origin of its port, and acknowledges once all is written', async () => {
    const { target } = memoryTarget();
    const { opener, deliver, finish } = page(target);
    expect(opener.postMessage.mock.calls.map(([, origin]) => origin)).toEqual([
      `http://localhost:${PORT}`, `http://127.0.0.1:${PORT}`, `http://[::1]:${PORT}`,
    ]);
    opener.postMessage.mockClear();

    deliver(exportOf([{ workspaceId: 'dev', unsentMessages: [unsentRecord('e1')], pendingUploads: [] }]));
    await settle();
    expect(opener.postMessage).toHaveBeenCalledWith({ kind: OUTBOX_HANDOFF_MESSAGES.ack, imported: 1 }, SOURCE);
    expect(finish).toHaveBeenCalledOnce();
  });

  it('hears nobody but its opener on a loopback origin of its own port', async () => {
    const { target, unsent } = memoryTarget();
    const { opener, deliver } = page(target);
    opener.postMessage.mockClear();
    const exported = exportOf([{ workspaceId: 'dev', unsentMessages: [unsentRecord('e1')], pendingUploads: [] }]);
    deliver(exported, 'http://localhost:9999');
    deliver(exported, 'https://evil.example');
    deliver(exported, SOURCE, { postMessage: vi.fn() });
    await settle();
    expect(unsent).toEqual([]);
    expect(opener.postMessage).not.toHaveBeenCalled();
  });

  it('refuses out loud when a write fails, and takes a retry', async () => {
    const { target } = memoryTarget(true);
    const { opener, deliver, statuses, finish } = page(target);
    opener.postMessage.mockClear();
    const exported = exportOf([{ workspaceId: 'dev', unsentMessages: [unsentRecord('e1')], pendingUploads: [] }]);
    deliver(exported);
    await settle();
    expect(opener.postMessage).toHaveBeenCalledWith({ kind: OUTBOX_HANDOFF_MESSAGES.ack, error: 'quota exceeded' }, SOURCE);
    expect(statuses[statuses.length - 1]).toContain('quota exceeded');
    expect(finish).not.toHaveBeenCalled();

    deliver(exported);
    await settle();
    expect(opener.postMessage).toHaveBeenCalledTimes(2);
  });

  it('refuses an export it cannot read rather than leaving the hand-off waiting', () => {
    const { target } = memoryTarget();
    const { opener, deliver } = page(target);
    opener.postMessage.mockClear();
    deliver({ kind: OUTBOX_HANDOFF_MESSAGES.export, version: 99, workspaces: [] });
    expect(opener.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ kind: OUTBOX_HANDOFF_MESSAGES.ack, error: expect.any(String) }), SOURCE);
  });

  it('leaves for the picker at once when no hand-off opened it', () => {
    const { finish } = page(memoryTarget().target, false);
    expect(finish).toHaveBeenCalledOnce();
  });
});

describe('native outbox import', () => {
  /** A payload as the hand-off page stores it: bytes `[1, 2, 250, 255]` in base64. */
  const payload = (eventId: string) => JSON.stringify(exportOf([{
    workspaceId: 'dev',
    unsentMessages: [unsentRecord(eventId)],
    pendingUploads: [{ ...uploadRecord('u1'), bytes: 'AQL6/w==' as unknown as ArrayBuffer }],
  }]));

  function desktop(payloads: unknown) {
    return vi.fn(async (cmd: string) => (cmd === OUTBOX_HANDOFF_COMMANDS.take ? payloads : undefined));
  }

  it('imports every stored payload, then acknowledges exactly those', async () => {
    const { target, unsent, uploads } = memoryTarget();
    const invoke = desktop([payload('e1'), payload('e2')]);
    expect(await importNativeOutbox(invoke, target)).toBe(4);
    expect(unsent.map(([, r]) => [r.eventId, r.phase])).toEqual([['e1', 'unsent'], ['e2', 'unsent']]);
    expect([...new Uint8Array(uploads[0][1].bytes)]).toEqual([1, 2, 250, 255]);
    expect(invoke).toHaveBeenLastCalledWith(OUTBOX_HANDOFF_COMMANDS.acknowledge, {
      payloads: [payload('e1'), payload('e2')],
    });
  });

  it('acknowledges nothing when nothing is stored', async () => {
    const invoke = desktop([]);
    expect(await importNativeOutbox(invoke, memoryTarget().target)).toBe(0);
    expect(invoke).toHaveBeenCalledOnce();
  });

  it('keeps the stored payloads when a write fails or a payload is unreadable', async () => {
    for (const [payloads, fail] of [[[payload('e1')], true], [['not json'], false]] as const) {
      const invoke = desktop(payloads);
      await expect(importNativeOutbox(invoke, memoryTarget(fail).target)).rejects.toThrow();
      expect(invoke).not.toHaveBeenCalledWith(OUTBOX_HANDOFF_COMMANDS.acknowledge, expect.anything());
    }
  });
});
