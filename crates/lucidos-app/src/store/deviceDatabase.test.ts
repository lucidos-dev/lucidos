import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  DEVICE_DATABASE_MAX_STALL_MS, DEVICE_DATABASE_STALL_MS, DeviceDatabaseReleasedError, DeviceDatabaseStalledError,
  deviceDatabaseName, openDeviceDatabase, releaseDeviceDatabases, rerunWhenPageShows,
} from './deviceDatabase';

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
  closed: boolean;
  transaction: () => FakeTx;
  transactions: FakeTx[];
}

function fakeDb(): FakeDb {
  const db: FakeDb = {
    onversionchange: null,
    onclose: null,
    close: vi.fn(() => { db.closed = true; }),
    closed: false,
    transactions: [],
    transaction: () => {
      if (db.closed) throw new DOMException('The database connection is closing', 'InvalidStateError');
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

  it('fails a transaction whose commit began at its bound but never finished', async () => {
    const db = openDeviceDatabase('test', () => {});
    const pending = db.transact('s', 'readwrite', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[0].result.transactions[0].abort.mockImplementation(() => {
      throw new DOMException('committing', 'InvalidStateError');
    });
    const settled = expect(pending).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
    expect(opens[0].result.close).toHaveBeenCalledOnce();
  });

  it('opens a fresh connection after a bounded transaction stalls', async () => {
    const db = openDeviceDatabase('test', () => {});
    const first = db.transact('s', 'readonly', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const settled = expect(first).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
    expect(opens[0].result.close).toHaveBeenCalledOnce();

    const second = db.transact('s', 'readonly', read, { bounded: true });
    expect(opens).toHaveLength(2);
    opens[1].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[1].result.transactions[0].oncomplete!();
    await expect(second).resolves.toBe('value');
  });

  it('keeps a retired connection\'s late events from dropping its replacement', async () => {
    const db = openDeviceDatabase('test', () => {});
    const first = db.transact('s', 'readonly', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const settled = expect(first).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
    const second = db.transact('s', 'readonly', read);
    opens[1].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[1].result.transactions[0].oncomplete!();
    await second;

    opens[0].result.onclose?.();
    opens[0].result.onversionchange?.();
    opens[0].result.transactions[0].onabort?.();
    const third = db.transact('s', 'readonly', read);
    await vi.advanceTimersByTimeAsync(0);
    expect(opens).toHaveLength(2);
    opens[1].result.transactions[1].oncomplete!();
    await expect(third).resolves.toBe('value');
  });

  it('lets an unbounded write on a retired connection finish', async () => {
    const db = openDeviceDatabase('test', () => {});
    const write = db.transact('s', 'readwrite', read);
    const stalledRead = db.transact('s', 'readonly', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const settled = expect(stalledRead).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
    const [writeTx] = opens[0].result.transactions;
    expect(writeTx.abort).not.toHaveBeenCalled();
    writeTx.oncomplete!();
    await expect(write).resolves.toBe('value');
  });

  it('reopens for an operation that reached a connection after it was retired', async () => {
    const db = openDeviceDatabase('test', () => {});
    const stalledRead = db.transact('s', 'readonly', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    let write: Promise<unknown> | undefined;
    opens[0].result.transactions[0].abort.mockImplementation(() => {
      write = db.transact('s', 'readwrite', read);
    });
    const settled = expect(stalledRead).rejects.toBeInstanceOf(DeviceDatabaseStalledError);
    await vi.advanceTimersByTimeAsync(DEVICE_DATABASE_STALL_MS);
    await settled;
    expect(opens).toHaveLength(2);
    opens[1].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[1].result.transactions[0].oncomplete!();
    await expect(write).resolves.toBe('value');
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

describe('releasing device storage when the page hides', () => {
  it('aborts a bounded transaction, keeps an unbounded write, and closes the connection', async () => {
    const db = openDeviceDatabase('test', () => {});
    const write = db.transact('s', 'readwrite', read);
    const step = db.transact('s', 'readonly', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    const [writeTx, stepTx] = opens[0].result.transactions;
    const released = expect(step).rejects.toBeInstanceOf(DeviceDatabaseReleasedError);
    vi.advanceTimersByTime(1_234);

    releaseDeviceDatabases();
    await released;
    expect(stepTx.abort).toHaveBeenCalledOnce();
    expect(writeTx.abort).not.toHaveBeenCalled();
    expect(opens[0].result.close).toHaveBeenCalledOnce();
    writeTx.oncomplete!();
    await expect(write).resolves.toBe('value');

    const [, init] = vi.mocked(fetch).mock.lastCall!;
    expect(JSON.parse(init!.body as string)).toEqual(expect.objectContaining({
      category: 'device-database',
      message: 'released_on_hide',
      data: { transactions: [
        { database: 'test', mode: 'readwrite', bounded: false, age_ms: 1_234 },
        { database: 'test', mode: 'readonly', bounded: true, age_ms: 1_234 },
      ] },
    }));

    const next = db.transact('s', 'readonly', read, { bounded: true });
    expect(opens).toHaveLength(2);
    opens[1].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[1].result.transactions[0].oncomplete!();
    await expect(next).resolves.toBe('value');
  });

  it('fails a bounded transaction whose abort never answers, and closes an idle connection quietly', async () => {
    const wedged = openDeviceDatabase('wedged', () => {});
    const step = wedged.transact('s', 'readonly', read, { bounded: true });
    opens[0].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[0].result.transactions[0].abort.mockImplementation(() => {});
    const released = expect(step).rejects.toBeInstanceOf(DeviceDatabaseReleasedError);
    releaseDeviceDatabases();
    await released;

    const idle = openDeviceDatabase('idle', () => {});
    void idle.transact('s', 'readonly', read).then(() => {}, () => {});
    opens[1].onsuccess!();
    await vi.advanceTimersByTimeAsync(0);
    opens[1].result.transactions[0].oncomplete!();
    await vi.advanceTimersByTimeAsync(0);
    vi.mocked(fetch).mockClear();
    releaseDeviceDatabases();
    await Promise.resolve();
    expect(opens[1].result.close).toHaveBeenCalledOnce();
    expect(fetch).not.toHaveBeenCalled();
  });
});

describe('rerunWhenPageShows', () => {
  it('reads again once the page shows, and only for a release', () => {
    const run = vi.fn();
    expect(rerunWhenPageShows(new DeviceDatabaseStalledError('db', 'transaction'), run)).toBe(false);
    expect(rerunWhenPageShows(new DeviceDatabaseReleasedError('db'), run)).toBe(true);
    expect(run).not.toHaveBeenCalled();
    window.dispatchEvent(new Event('pageshow'));
    window.dispatchEvent(new Event('pageshow'));
    expect(run).toHaveBeenCalledOnce();
  });
});

describe('deviceDatabaseName', () => {
  it('scopes a peer-served client apart from the own client', () => {
    expect(deviceDatabaseName('lucidos-unsent-messages', 'dev@cloud', null)).toBe('lucidos-unsent-messages:dev@cloud');
    expect(deviceDatabaseName('lucidos-unsent-messages', 'dev@cloud', 'cloud')).toBe(
      'peer:cloud:lucidos-unsent-messages:dev@cloud',
    );
  });
});
