import { describe, expect, it, vi } from 'vitest';
import handoffScript from '../../../public/outbox-handoff.js?raw';
import {
  OUTBOX_HANDOFF_COMMANDS,
  OUTBOX_HANDOFF_MESSAGES,
  OUTBOX_IMPORT_PATH,
  decodeNativeOutbox,
  type OutboxExport,
} from '../outboxHandoff';
import { unsentRecord, uploadRecord } from './fixtures';

interface Action { label: string; onPress: () => void }
interface Handoff {
  names: { clearPath: string };
  exportOutbox(idb: unknown): Promise<OutboxExport>;
  start(overrides: object): Promise<void>;
}

function load(): Handoff {
  const window: { lucidosOutboxHandoff?: Handoff } = {};
  new Function('window', handoffScript)(window);
  return window.lucidosOutboxHandoff!;
}

type FakeStore = unknown[] | { keys: unknown[]; values: unknown[] };

function fakeIdb(dbs: Record<string, Record<string, FakeStore>>, listed = Object.keys(dbs)) {
  return {
    databases: async () => listed.map((name) => ({ name, version: 1 })),
    open(name: string) {
      const request: Record<string, unknown> & { onsuccess?: () => void; onerror?: () => void; onupgradeneeded?: () => void } = {};
      setTimeout(() => {
        const stores = dbs[name];
        if (!stores) {
          request.transaction = { abort: () => {} };
          request.onupgradeneeded?.();
          request.onerror?.();
          return;
        }
        request.result = {
          objectStoreNames: { contains: (store: string) => store in stores },
          close: () => {},
          transaction: (store: string) => {
            const transaction: { oncomplete?: () => void; objectStore?: unknown } = {};
            const held = stores[store];
            const values = Array.isArray(held) ? held : held.values;
            const keys = Array.isArray(held) ? held.map((_, i) => i) : held.keys;
            const later = (result: unknown[], then?: () => void) => {
              const request: { result?: unknown[] } = {};
              setTimeout(() => {
                request.result = result;
                then?.();
              });
              return request;
            };
            transaction.objectStore = () => ({
              getAllKeys: () => later(keys),
              getAll: () => later(values, () => transaction.oncomplete?.()),
            });
            return transaction;
          },
        };
        request.onsuccess?.();
      });
      return request;
    },
  };
}

const PORT = '5252';
const TARGET = `https://localhost:${PORT}`;
const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

function harness(
  idb: unknown,
  { blocked = false, clearStatus = 200, invoke = null as ((cmd: string, args: unknown) => Promise<unknown>) | null } = {},
) {
  const opened = { postMessage: vi.fn(), closed: false };
  const checks = new Set<() => void>();
  const listeners = new Set<(event: MessageEvent) => void>();
  const rendered: { text: string; action: Action | null }[] = [];
  const deps = {
    indexedDB: idb,
    location: { port: PORT, pathname: '/dev/', search: '?q=1', hash: '#t', replace: vi.fn() },
    open: vi.fn(() => (blocked ? null : opened)),
    every: (check: () => void) => {
      checks.add(check);
      return () => checks.delete(check);
    },
    fetch: vi.fn(async () => ({ ok: clearStatus === 200, status: clearStatus })),
    invoke,
    addMessageListener: (l: (event: MessageEvent) => void) => listeners.add(l),
    removeMessageListener: (l: (event: MessageEvent) => void) => listeners.delete(l),
    render: (text: string, action: Action | null) => rendered.push({ text, action }),
  };
  const fromImport = (data: unknown, origin = TARGET, source: unknown = opened) => {
    for (const l of [...listeners]) l({ data, origin, source } as unknown as MessageEvent);
  };
  const last = () => rendered[rendered.length - 1];
  const tick = () => { for (const check of [...checks]) check(); };
  return { deps, opened, fromImport, rendered, last, tick, checks };
}

const outbox = () => fakeIdb({
  'lucidos-unsent-messages:dev': { messages: [unsentRecord('e1')] },
  'lucidos-pending-uploads:dev': { uploads: [uploadRecord('u1')] },
  'lucidos-unsent-messages': { messages: [unsentRecord('e2')] },
  'lucidos-restore-generation:dev': { generations: [{}] },
});

describe('outbox hand-off page', () => {
  it('exports the outbox per workspace and nothing else', async () => {
    const exported = await load().exportOutbox(fakeIdb(
      {
        'lucidos-unsent-messages:dev': { messages: [unsentRecord('e1')] },
        'lucidos-pending-uploads:dev': { uploads: [uploadRecord('u1')] },
        'lucidos-unsent-messages': { messages: [unsentRecord('e2')] },
        'lucidos-pending-uploads:empty': { uploads: [] },
        'lucidos-restore-generation:dev': { generations: [{}] },
      },
      ['lucidos-unsent-messages:dev', 'lucidos-pending-uploads:dev', 'lucidos-unsent-messages',
        'lucidos-pending-uploads:empty', 'lucidos-restore-generation:dev', 'lucidos-unsent-messages:gone'],
    ));
    expect(exported.kind).toBe(OUTBOX_HANDOFF_MESSAGES.export);
    expect(exported.workspaces.map((w) => [w.workspaceId, w.unsentMessages.length, w.pendingUploads.length]))
      .toEqual([['dev', 1, 1], [null, 1, 0]]);
  });

  it('carries a restore journal with its keys, and counts each observation', async () => {
    const observation = { generation: 3, recoveries: [] };
    const exported = await load().exportOutbox(fakeIdb({
      'lucidos-restore-generation:dev': {
        observations: { keys: ['workspace'], values: [observation] },
        'upload-bytes': { keys: [[['f', 1], 0]], values: [{ bytes: new ArrayBuffer(2) }] },
      },
      'lucidos-restore-generation:empty': { observations: [], 'upload-bytes': [] },
    }));
    expect(exported.workspaces).toEqual([{
      workspaceId: 'dev',
      unsentMessages: [],
      pendingUploads: [],
      restoreJournal: {
        observations: [['workspace', observation]],
        uploadBytes: [[[['f', 1], 0], { bytes: expect.any(ArrayBuffer) }]],
      },
    }]);
  });

  it('never clears an origin whose only saved work is a restore journal', async () => {
    const h = harness(fakeIdb({
      'lucidos-restore-generation:dev': {
        observations: { keys: ['workspace'], values: [{ generation: 3, recoveries: [] }] },
        'upload-bytes': [],
      },
    }));
    await load().start(h.deps);
    expect(h.deps.fetch).not.toHaveBeenCalled();
    expect(h.last().text).toContain('1 unsent item');
  });

  it('with nothing unsent, clears this origin and moves to https at once', async () => {
    const { deps } = harness(fakeIdb({}));
    await load().start(deps);
    expect(deps.fetch).toHaveBeenCalledWith(load().names.clearPath, { method: 'POST' });
    expect(deps.location.replace).toHaveBeenCalledWith(`${TARGET}/dev/?q=1#t`);
  });

  it('clears only after the import acknowledged every record', async () => {
    const h = harness(outbox());
    await load().start(h.deps);
    expect(h.last().text).toContain('3 unsent items');
    h.last().action!.onPress();
    expect(h.deps.open).toHaveBeenCalledWith(`${TARGET}${OUTBOX_IMPORT_PATH}`);

    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ready });
    expect(h.opened.postMessage).toHaveBeenCalledWith(expect.objectContaining({ kind: OUTBOX_HANDOFF_MESSAGES.export }), TARGET);
    expect(h.deps.fetch).not.toHaveBeenCalled();

    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ack, imported: 3 });
    await settle();
    expect(h.deps.fetch).toHaveBeenCalledOnce();
    expect(h.last().text).toContain('moved');
    expect(h.deps.location.replace).not.toHaveBeenCalled();
  });

  it('keeps everything when the import fails or falls short, and offers again', async () => {
    for (const ack of [{ error: 'quota exceeded' }, { imported: 2 }]) {
      const h = harness(outbox());
      await load().start(h.deps);
      h.last().action!.onPress();
      h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ready });
      h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ack, ...ack });
      await settle();
      expect(h.deps.fetch).not.toHaveBeenCalled();
      expect(h.last().text).toContain('still here');
      expect(h.last().action?.label).toBe('Continue');
    }
  });

  it('offers Continue again when the new window closes before it answers', async () => {
    const h = harness(outbox());
    await load().start(h.deps);
    h.last().action!.onPress();
    h.tick();
    expect(h.last().text).toContain('Finish in the new window');
    h.opened.closed = true;
    h.tick();
    await settle();
    expect(h.last().text).toContain('still here');
    expect(h.last().action?.label).toBe('Continue');
    expect(h.checks.size).toBe(0);
    expect(h.deps.fetch).not.toHaveBeenCalled();
  });

  it('stops watching the new window once it answers', async () => {
    const h = harness(outbox());
    await load().start(h.deps);
    h.last().action!.onPress();
    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ready });
    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ack, imported: 3 });
    await settle();
    expect(h.checks.size).toBe(0);
  });

  it('hears only the window it opened, on the https origin', async () => {
    const h = harness(outbox());
    await load().start(h.deps);
    h.last().action!.onPress();
    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ready }, 'https://evil.example');
    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ready }, TARGET, { postMessage: vi.fn() });
    h.fromImport({ kind: OUTBOX_HANDOFF_MESSAGES.ack, imported: 3 }, 'http://localhost:5252');
    await settle();
    expect(h.opened.postMessage).not.toHaveBeenCalled();
    expect(h.deps.fetch).not.toHaveBeenCalled();
  });

  it('says so when the browser blocks the new window', async () => {
    const h = harness(outbox(), { blocked: true });
    await load().start(h.deps);
    h.last().action!.onPress();
    await settle();
    expect(h.last().text).toContain('blocked the new window');
    expect(h.deps.fetch).not.toHaveBeenCalled();
  });

  it('in the desktop window, stores the outbox natively, then clears and moves on', async () => {
    const upload = { ...uploadRecord('u1'), bytes: new Uint8Array([1, 2, 250, 255]).buffer };
    const invoke = vi.fn(async () => undefined);
    const h = harness(fakeIdb({
      'lucidos-unsent-messages:dev': { messages: [unsentRecord('e1')] },
      'lucidos-pending-uploads:dev': { uploads: [upload] },
    }), { invoke });
    await load().start(h.deps);

    expect(invoke).toHaveBeenCalledOnce();
    const [command, args] = invoke.mock.calls[0] as unknown as [string, { payload: string }];
    expect(command).toBe(OUTBOX_HANDOFF_COMMANDS.store);
    const decoded = decodeNativeOutbox(args.payload);
    expect(decoded?.workspaces[0].unsentMessages).toEqual([unsentRecord('e1')]);
    expect([...new Uint8Array(decoded!.workspaces[0].pendingUploads[0].bytes)]).toEqual([1, 2, 250, 255]);
    expect(h.deps.open).not.toHaveBeenCalled();
    expect(h.deps.fetch).toHaveBeenCalledOnce();
    expect(h.deps.location.replace).toHaveBeenCalledWith(`${TARGET}/dev/?q=1#t`);
  });

  it('in the desktop window, stores no payload when nothing is unsent', async () => {
    const invoke = vi.fn(async () => undefined);
    const h = harness(fakeIdb({}), { invoke });
    await load().start(h.deps);
    expect(invoke).toHaveBeenCalledWith(OUTBOX_HANDOFF_COMMANDS.store, { payload: null });
    expect(h.deps.location.replace).toHaveBeenCalledWith(`${TARGET}/dev/?q=1#t`);
  });

  it('in the desktop window, clears nothing when the native store refuses', async () => {
    const h = harness(outbox(), { invoke: vi.fn(async () => { throw 'disk full'; }) });
    await load().start(h.deps);
    expect(h.deps.fetch).not.toHaveBeenCalled();
    expect(h.deps.location.replace).not.toHaveBeenCalled();
    expect(h.last().text).toBe('disk full. Nothing unsent was lost.');
    h.last().action!.onPress();
    expect(h.deps.location.replace).toHaveBeenCalledWith(`${TARGET}/dev/?q=1#t`);
  });

  it('clears nothing when the browser cannot list its databases', async () => {
    const h = harness({});
    await load().start(h.deps);
    expect(h.deps.fetch).not.toHaveBeenCalled();
    expect(h.last().text).toContain('nothing was moved or cleared');
    h.last().action!.onPress();
    expect(h.deps.location.replace).toHaveBeenCalledWith(`${TARGET}/dev/?q=1#t`);
  });
});
