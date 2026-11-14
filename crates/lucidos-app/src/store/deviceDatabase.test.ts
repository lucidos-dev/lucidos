import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DEVICE_DATABASE_MAX_STALL_MS, DEVICE_DATABASE_STALL_MS, DeviceDatabaseStalledError, openDeviceDatabase,
} from './deviceDatabase';

/** A hand-driven IndexedDB: each open and transaction settles only when the test says. */
interface FakeOpen {
  onsuccess: (() => void) | null;
  onerror: (() => void) | null;
  onblocked: (() => void) | null;
  onupgradeneeded: (() => void) | null;
  result: FakeDb;
  error: DOMException | null;
  transaction: null;
}

interface FakeTx {
  oncomplete: (() => void) | null;
  onerror: (() => void) | null;
  onabort: (() => void) | null;
  error: DOMException | null;
  abort: ReturnType<typeof vi.fn>;
  objectStore: () => { get: () => { result: string } };
}

interface FakeDb {
  onversionchange: (() => void) | null;
  onclose: (() => void) | null;
  close: ReturnType<typeof vi.fn>;
  transaction: () => FakeTx;
  transactions: FakeTx[];
}

function fakeDb(): FakeDb {
  const db: FakeDb = {
    onversionchange: null,
    onclose: null,
    close: vi.fn(),
    transactions: [],
    transaction: () => {
      const tx: FakeTx = {
        oncomplete: null,
        onerror: null,
        onabort: null,
        error: null,
        abort: vi.fn(() => {
          tx.error = new DOMException('aborted', 'AbortError');
          tx.onabort?.();
        }),
        objectStore: () => ({ get: () => ({ result: 'value' }) }),
      };
      db.transactions.push(tx);
      return tx;
    },
  };
  return db;
}

let opens: FakeOpen[];

beforeEach(() => {
  vi.useFakeTimers();
  opens = [];
  vi.stubGlobal('indexedDB', {
    open: () => {
      const req: FakeOpen = {
        onsuccess: null, onerror: null, onblocked: null, onupgradeneeded: null,
        result: fakeDb(), error: null, transaction: null,
      };
      opens.push(req);
      return req;
    },
  });
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response())));
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const read = (s: IDBObjectStore) => s.get('key') as IDBRequest<string>;

describe('openDeviceDatabase', () => {
  it('drops a stalled open, so the next call opens afresh', async () => {
    const db = openDeviceDatabase('test', () => {});
    const first = db.transact('s', 'readonly', read);
    const settled = expect(first).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;

    const second = db.transact('s', 'readonly', read);
    expect(opens).toHaveLength(2);
    opens[1].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[1].result.transactions[0].oncomplete!();
    await expect(second).resolves.toBe('value');
  });

  it('closes a connection that lands after its open stalled', async () => {
    const db = openDeviceDatabase('test', () => {});
    const first = db.transact('s', 'readonly', read);
    const settled = expect(first).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;

    opens[0].onsuccess!();
    expect(opens[0].result.close).toHaveBeenCalledOnce();
  });

  it('reports a blocked open to the engine log', () => {
    const db = openDeviceDatabase('test', () => {});
    void db.transact('s', 'readonly', read).catch(() => {});
    opens[0].onblocked!();
    const [, init] = vi.mocked(fetch).mock.calls[0];
    expect(JSON.parse(init!.body as string)).toMatchObject({
      category: 'device-database',
      message: 'open_blocked',
      data: { name: 'test' },
    });
  });

  it('aborts a bounded transaction that stalls', async () => {
    const db = openDeviceDatabase('test', () => {});
    const pending = db.transact('s', 'readwrite', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const settled = expect(pending).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
    expect(opens[0].result.transactions[0].abort).toHaveBeenCalledOnce();
  });

  // A wedged backend can take the abort and never dispatch its event.
  it('fails a stalled transaction whose abort event never comes', async () => {
    const db = openDeviceDatabase('test', () => {});
    const pending = db.transact('s', 'readwrite', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[0].result.transactions[0].abort.mockImplementation(() => {});
    const settled = expect(pending).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
  });

  // Its commit beat the timer, so it succeeded and the bound stays as it was.
  it('lets a transaction that finished at its bound settle by its own event', async () => {
    const db = openDeviceDatabase('test', () => {});
    const pending = db.transact('s', 'readwrite', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const tx = opens[0].result.transactions[0];
    tx.abort.mockImplementation(() => { throw new DOMException('finished', 'InvalidStateError'); });
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    tx.oncomplete!();
    await expect(pending).resolves.toBe('value');

    const next = db.transact('s', 'readonly', read, { bounded: true });
    await vi.advanceTimersByTimeAsync(0);
    const settled = expect(next).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
  });

  it('resolves a bounded transaction that completes in time, and arms nothing after', async () => {
    const db = openDeviceDatabase('test', () => {});
    const pending = db.transact('s', 'readwrite', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const tx = opens[0].result.transactions[0];
    tx.oncomplete!();
    await expect(pending).resolves.toBe('value');
    expect(vi.getTimerCount()).toBe(0);
    expect(tx.abort).not.toHaveBeenCalled();
  });

  it('doubles the bound after each stall, up to its cap', async () => {
    const db = openDeviceDatabase('test', () => {});
    const stallAfter = async (ms: number) => {
      const pending = db.transact('s', 'readonly', read);
      const settled = expect(pending).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
      await vi.advanceTimersByTimeAsync(ms - 1);
      expect(vi.getTimerCount()).toBe(1);
      await vi.advanceTimersByTimeAsync(1);
      await settled;
    };
    let bound = DEVICE_DATABASE_STALL_MS;
    while (bound < DEVICE_DATABASE_MAX_STALL_MS) {
      await stallAfter(bound);
      bound *= 2;
    }
    await stallAfter(DEVICE_DATABASE_MAX_STALL_MS);
    await stallAfter(DEVICE_DATABASE_MAX_STALL_MS);
  });

  it('leaves an unbounded transaction to run as long as it needs', async () => {
    const db = openDeviceDatabase('test', () => {});
    const pending = db.transact('s', 'readwrite', read);
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS * 4);
    const tx = opens[0].result.transactions[0];
    expect(tx.abort).not.toHaveBeenCalled();
    tx.oncomplete!();
    await expect(pending).resolves.toBe('value');
  });
});
