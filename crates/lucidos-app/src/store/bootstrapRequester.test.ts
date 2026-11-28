import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ApiError } from '../api/client';
import type { BootstrapRead, BootstrapSliceKey, BootstrapSnapshot } from '../api/client/bootstrap';
import { loadSlice, type BootstrapSlice } from './bootstrapSliceLoads';
import { createBootstrapRequester, sliceEntry, type BootstrapSliceTable } from './bootstrapRequester';

const KEYS = ['preferences', 'users', 'devices'] as const satisfies readonly BootstrapSliceKey[];
type Key = typeof KEYS[number];

let log: string[];
let narrowReads: Record<Key, ReturnType<typeof vi.fn>>;
let slices: Record<Key, BootstrapSlice<unknown, number>>;
let reads: Array<{ etag: string | null; settle: (read: BootstrapRead) => void; reject: (error: unknown) => void }>;
let bootstrap: ReturnType<typeof createBootstrapRequester>;

const loadNarrow = (key: Key) => loadSlice(key, slices[key] as never, narrowReads[key] as never);

function snapshotOf(results: Record<Key, unknown>): BootstrapSnapshot {
  return results as unknown as BootstrapSnapshot;
}

const DATA = snapshotOf({ preferences: { data: 'p1' }, users: { data: 'u1' }, devices: { data: 'd1' } });

beforeEach(() => {
  log = [];
  reads = [];
  narrowReads = { preferences: vi.fn(), users: vi.fn(), devices: vi.fn() };
  slices = {} as typeof slices;
  const table: Record<string, unknown> = {};
  for (const key of KEYS) {
    let begun = 0;
    narrowReads[key].mockImplementation(() => Promise.resolve(`${key}-narrow`));
    slices[key] = {
      begin: () => ++begun,
      apply: (body, ctx) => { log.push(`apply ${key}#${ctx} ${String(body)}`); },
      fail: (error, ctx) => {
        const code = error instanceof ApiError ? ` ${error.httpCode}` : '';
        log.push(`fail ${key}#${ctx}${code} ${(error as Error).message}`);
      },
    };
    table[key] = sliceEntry(slices[key], () => loadNarrow(key));
  }
  bootstrap = createBootstrapRequester(
    table as unknown as BootstrapSliceTable,
    (etag) => new Promise<BootstrapRead>((settle, reject) => { reads.push({ etag, settle, reject }); }),
  );
});

describe('requestBootstrap', () => {
  it('applies every slice, in table order, with the context its begin returned', async () => {
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: 'W/"1"' });
    await done;
    expect(log).toEqual(['apply preferences#1 p1', 'apply users#1 u1', 'apply devices#1 d1']);
  });

  it('fails one slice with its own HTTP code and still applies the rest', async () => {
    const done = bootstrap.requestBootstrap();
    reads[0].settle({
      kind: 'snapshot',
      etag: null,
      snapshot: snapshotOf({
        preferences: { data: 'p1' },
        users: { error: { message: 'no such table', http_code: 500 } },
        devices: { data: 'd1' },
      }),
    });
    await done;
    expect(log).toEqual(['apply preferences#1 p1', 'fail users#1 500 500 no such table', 'apply devices#1 d1']);
  });

  it('leaves no start behind when a begin throws, so an older failure still fails its slice', async () => {
    let failNarrow = (_: Error) => {};
    narrowReads.preferences.mockImplementation(() => new Promise((_, reject) => { failNarrow = reject; }));
    const narrow = loadNarrow('preferences');
    slices.devices.begin = () => { throw new Error('devices begin'); };
    await expect(bootstrap.requestBootstrap()).rejects.toThrow('devices begin');
    failNarrow(new Error('down'));
    await narrow;
    expect(log).toEqual(['fail users#1 devices begin', 'fail preferences#1 down']);
  });

  it('fails every slice that began before a begin threw, so none stays loading', async () => {
    slices.devices.begin = () => { throw new Error('devices begin'); };
    await expect(bootstrap.requestBootstrap()).rejects.toThrow('devices begin');
    expect(log).toEqual(['fail preferences#1 devices begin', 'fail users#1 devices begin']);
  });

  it('fails only the slice whose error is malformed', async () => {
    const done = bootstrap.requestBootstrap();
    reads[0].settle({
      kind: 'snapshot',
      etag: null,
      snapshot: snapshotOf({ preferences: { error: null }, users: { data: 'u1' }, devices: { data: 'd1' } }),
    });
    await done;
    expect(log[0]).toMatch(/^fail preferences#1 /);
    expect(log.slice(1)).toEqual(['apply users#1 u1', 'apply devices#1 d1']);
  });

  it('hands a slice whose apply throws to its fail', async () => {
    slices.users.apply = () => { throw new Error('bad body'); };
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await done;
    expect(log).toContain('fail users#1 bad body');
    expect(log).toContain('apply devices#1 d1');
  });

  it('fails every slice when the request fails, and never falls back to the loaders', async () => {
    const done = bootstrap.requestBootstrap();
    reads[0].reject(new ApiError(503, 'Lucidos is restarting'));
    await done;
    expect(log).toEqual([
      'fail preferences#1 503 503 Lucidos is restarting',
      'fail users#1 503 503 Lucidos is restarting',
      'fail devices#1 503 503 Lucidos is restarting',
    ]);
    for (const key of KEYS) expect(narrowReads[key]).not.toHaveBeenCalled();
  });

  it('runs every slice loader once when the route is missing, and fails none', async () => {
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'route-missing' });
    await done;
    for (const key of KEYS) expect(narrowReads[key]).toHaveBeenCalledTimes(1);
    expect(log.filter((line) => line.startsWith('fail'))).toEqual([]);
    expect(log).toHaveLength(KEYS.length);
  });

  it('asks again with the ETag it holds, and a 304 re-applies the snapshot it holds', async () => {
    const first = bootstrap.requestBootstrap();
    expect(reads[0].etag).toBeNull();
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: 'W/"1"' });
    await first;
    log.length = 0;

    const second = bootstrap.requestBootstrap();
    expect(reads[1].etag).toBe('W/"1"');
    reads[1].settle({ kind: 'not-modified' });
    await second;
    expect(log).toEqual(['apply preferences#2 p1', 'apply users#2 u1', 'apply devices#2 d1']);
  });

  it('loads a slice the snapshot lacks from its own route, after the rest applied', async () => {
    for (const key of KEYS) slices[key].abandon = (ctx) => { log.push(`abandon ${key}#${ctx}`); };
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'snapshot', snapshot: snapshotOf({ preferences: { data: 'p1' }, devices: { data: 'd1' } } as never), etag: null });
    await done;
    expect(log).toEqual([
      'apply preferences#1 p1', 'apply devices#1 d1', 'abandon users#1', 'apply users#2 users-narrow',
    ]);
  });

  it('fails every slice when a 304 arrives before any snapshot', async () => {
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'not-modified' });
    await done;
    expect(log).toHaveLength(KEYS.length);
    for (const line of log) expect(line).toMatch(/^fail \w+#1 The bootstrap answered "not modified"/);
  });

  it('skips a slice whose narrow read started while the bootstrap was out', async () => {
    const done = bootstrap.requestBootstrap();
    await loadNarrow('users');
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await done;
    expect(log).toEqual(['apply users#2 users-narrow', 'apply preferences#1 p1', 'apply devices#1 d1']);
  });

  it('keeps the snapshot when a narrow read that started before the bootstrap lands after it', async () => {
    let landOlder!: (body: string) => void;
    narrowReads.users.mockImplementationOnce(() => new Promise((resolve) => { landOlder = resolve; }));
    const older = loadNarrow('users');
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await done;
    landOlder('users-older');
    await older;
    expect(log).toEqual(['apply preferences#1 p1', 'apply users#2 u1', 'apply devices#1 d1']);
  });

  it('lets an older read the route-missing fallback joins settle the slice', async () => {
    let landOlder!: (body: string) => void;
    narrowReads.users.mockImplementationOnce(() => new Promise((resolve) => { landOlder = resolve; }));
    const older = loadNarrow('users');
    const joining = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, () => loadNarrow('preferences')),
      users: sliceEntry(slices.users, () => older),
      devices: sliceEntry(slices.devices, () => loadNarrow('devices')),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({ kind: 'route-missing' }));

    const done = joining.requestBootstrap();
    await vi.waitFor(() => expect(narrowReads.devices).toHaveBeenCalled());
    landOlder('users-older');
    await done;
    expect(log).toContain('apply users#1 users-older');
  });

  it('lets an older read the route-missing fallback joins fail the slice', async () => {
    let failOlder!: (error: Error) => void;
    narrowReads.users.mockImplementationOnce(() => new Promise((_, reject) => { failOlder = reject; }));
    const older = loadNarrow('users');
    const joining = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, () => loadNarrow('preferences')),
      users: sliceEntry(slices.users, () => older),
      devices: sliceEntry(slices.devices, () => loadNarrow('devices')),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({ kind: 'route-missing' }));

    const done = joining.requestBootstrap();
    await vi.waitFor(() => expect(narrowReads.devices).toHaveBeenCalled());
    failOlder(new Error('down'));
    await done;
    expect(log).toContain('fail users#1 down');
  });

  it('lets an older read fail the slice when the snapshot lacks it and the loader joins that read', async () => {
    let failOlder!: (error: Error) => void;
    narrowReads.users.mockImplementationOnce(() => new Promise((_, reject) => { failOlder = reject; }));
    const older = loadNarrow('users');
    const joining = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, () => loadNarrow('preferences')),
      users: sliceEntry(slices.users, () => older),
      devices: sliceEntry(slices.devices, () => loadNarrow('devices')),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({
      kind: 'snapshot', etag: null, snapshot: snapshotOf({ preferences: { data: 'p1' }, devices: { data: 'd1' } } as never),
    }));

    const done = joining.requestBootstrap();
    await vi.waitFor(() => expect(log).toContain('apply devices#1 d1'));
    failOlder(new Error('down'));
    await done;
    expect(log).toContain('fail users#1 down');
  });

  it('abandons the context of a slice a narrow read took over', async () => {
    slices.users.abandon = (ctx) => { log.push(`abandon users#${ctx}`); };
    const done = bootstrap.requestBootstrap();
    await loadNarrow('users');
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await done;
    expect(log).toEqual(['apply users#2 users-narrow', 'apply preferences#1 p1', 'abandon users#1', 'apply devices#1 d1']);
  });

  it('abandons every context it began before the loaders run, when the route is missing', async () => {
    for (const key of KEYS) slices[key].abandon = (ctx) => { log.push(`abandon ${key}#${ctx}`); };
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'route-missing' });
    await done;
    expect(log).toEqual([
      'abandon preferences#1', 'abandon users#1', 'abandon devices#1',
      'apply preferences#2 preferences-narrow', 'apply users#2 users-narrow', 'apply devices#2 devices-narrow',
    ]);
  });

  it('runs a loader named after another slice once that slice has loaded, when the route is missing', async () => {
    let finishPreferences = () => {};
    const load = (key: Key, wait?: Promise<void>) => async () => { await wait; log.push(`load ${key}`); };
    const ordered = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, load('preferences', new Promise<void>((resolve) => { finishPreferences = resolve; }))),
      users: sliceEntry(slices.users, load('users')),
      devices: sliceEntry(slices.devices, load('devices'), 'preferences'),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({ kind: 'route-missing' }));

    const done = ordered.requestBootstrap();
    await vi.waitFor(() => expect(log).toEqual(['load users']));
    finishPreferences();
    await done;
    expect(log).toEqual(['load users', 'load preferences', 'load devices']);
  });

  it('still runs a loader named after a slice whose loader rejected, when the route is missing', async () => {
    const ordered = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, () => Promise.reject(new Error('preferences loader'))),
      users: sliceEntry(slices.users, async () => {}),
      devices: sliceEntry(slices.devices, async () => { log.push('load devices'); }, 'preferences'),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({ kind: 'route-missing' }));

    await expect(ordered.requestBootstrap()).rejects.toThrow('preferences loader');
    expect(log).toEqual(['load devices']);
  });

  it('waits for every loader before it ends, when one rejects and the route is missing', async () => {
    let finishDevices = () => {};
    const ordered = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, () => Promise.reject(new Error('preferences loader'))),
      users: sliceEntry(slices.users, async () => {}),
      devices: sliceEntry(slices.devices, () => new Promise<void>((resolve) => { finishDevices = resolve; })),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({ kind: 'route-missing' }));

    const done = ordered.requestBootstrap();
    done.catch(() => {});
    await Promise.resolve();
    await Promise.resolve();
    ordered.holdGlobalFrame(() => { log.push('frame'); });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(log).toEqual([]);
    finishDevices();
    await expect(done).rejects.toThrow('preferences loader');
    expect(log).toEqual(['frame']);
  });

  it('starts every apply together, so a slice whose apply awaits delays no other', async () => {
    let finishUsers = () => {};
    slices.users.apply = (body, ctx) => new Promise<void>((resolve) => {
      finishUsers = () => { log.push(`apply users#${ctx} ${String(body)}`); resolve(); };
    });
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await vi.waitFor(() => expect(log).toEqual(['apply preferences#1 p1', 'apply devices#1 d1']));

    await loadNarrow('devices');
    finishUsers();
    await done;
    expect(log).toEqual(['apply preferences#1 p1', 'apply devices#1 d1', 'apply devices#2 devices-narrow', 'apply users#1 u1']);
  });

  it('still runs the later loaders when one throws before its promise, when the route is missing', async () => {
    const ordered = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, async () => { log.push('load preferences'); }),
      users: sliceEntry(slices.users, () => { throw new Error('users loader'); }),
      devices: sliceEntry(slices.devices, async () => { log.push('load devices'); }),
    } as unknown as BootstrapSliceTable, () => Promise.resolve({ kind: 'route-missing' }));

    await expect(ordered.requestBootstrap()).rejects.toThrow('users loader');
    expect(log).toEqual(['load preferences', 'load devices']);
  });

  it('still repeats for a second caller when the first pass rejects', async () => {
    const answers: BootstrapRead[] = [{ kind: 'route-missing' }, { kind: 'snapshot', snapshot: DATA, etag: null }];
    let failLoader = () => {};
    const ordered = createBootstrapRequester({
      preferences: sliceEntry(slices.preferences, () => new Promise<void>((_, reject) => {
        failLoader = () => reject(new Error('preferences loader'));
      })),
      users: sliceEntry(slices.users, async () => {}),
      devices: sliceEntry(slices.devices, async () => {}),
    } as unknown as BootstrapSliceTable, () => Promise.resolve(answers.shift()!));

    const first = ordered.requestBootstrap();
    await vi.waitFor(() => expect(answers).toHaveLength(1));
    const second = ordered.requestBootstrap();
    failLoader();
    await Promise.all([first, second]);
    expect(log).toEqual(['apply preferences#2 p1', 'apply users#2 u1', 'apply devices#2 d1']);
  });

  it('leaves a slice the snapshot lacks to a narrow read that started while the rest applied', async () => {
    let finishPreferences = () => {};
    slices.preferences.apply = () => new Promise<void>((resolve) => { finishPreferences = resolve; });
    slices.users.abandon = (ctx) => { log.push(`abandon users#${ctx}`); };
    const done = bootstrap.requestBootstrap();
    reads[0].settle({ kind: 'snapshot', snapshot: snapshotOf({ preferences: { data: 'p1' }, devices: { data: 'd1' } } as never), etag: null });
    await vi.waitFor(() => expect(log).toEqual(['apply devices#1 d1']));

    await loadNarrow('users');
    finishPreferences();
    await done;
    expect(log).toEqual(['apply devices#1 d1', 'apply users#2 users-narrow', 'abandon users#1']);
    expect(narrowReads.users).toHaveBeenCalledTimes(1);
  });

  it('never joins a running bootstrap: a second call repeats it after the first', async () => {
    const first = bootstrap.requestBootstrap();
    const second = bootstrap.requestBootstrap();
    void bootstrap.requestBootstrap();
    expect(reads).toHaveLength(1);

    let settled = false;
    void second.then(() => { settled = true; });
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: 'W/"1"' });
    await vi.waitFor(() => expect(reads).toHaveLength(2));
    expect(settled).toBe(false);

    reads[1].settle({ kind: 'not-modified' });
    await Promise.all([first, second]);
    expect(reads).toHaveLength(2);
    expect(log.filter((line) => line.startsWith('apply users'))).toEqual(['apply users#1 u1', 'apply users#2 u1']);
  });
});

describe('a global frame during a bootstrap', () => {
  it('is applied at once when no bootstrap is out', () => {
    bootstrap.holdGlobalFrame(() => log.push('frame'));
    expect(log).toEqual(['frame']);
  });

  it('waits, in order, until the snapshot has applied', async () => {
    const done = bootstrap.requestBootstrap();
    bootstrap.holdGlobalFrame(() => log.push('frame A'));
    bootstrap.holdGlobalFrame(() => log.push('frame B'));
    expect(log).toEqual([]);

    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await done;
    expect(log.slice(-3)).toEqual(['apply devices#1 d1', 'frame A', 'frame B']);
  });

  it('still applies the frames behind one that throws, and reports that one', async () => {
    const reported: Array<() => void> = [];
    const queue = vi.spyOn(globalThis, 'queueMicrotask').mockImplementation((task) => { reported.push(task); });
    try {
      const done = bootstrap.requestBootstrap();
      bootstrap.holdGlobalFrame(() => { throw new Error('bad frame'); });
      bootstrap.holdGlobalFrame(() => log.push('frame 2'));
      reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
      await done;
      expect(log[log.length - 1]).toBe('frame 2');
      expect(reported).toHaveLength(1);
      expect(reported[0]).toThrow('bad frame');
    } finally {
      queue.mockRestore();
    }
  });

  it('is released when the request fails too', async () => {
    const done = bootstrap.requestBootstrap();
    bootstrap.holdGlobalFrame(() => log.push('frame'));
    reads[0].reject(new Error('down'));
    await done;
    expect(log[log.length - 1]).toBe('frame');
  });
});

describe('a gap repair inside a bootstrap', () => {
  it('answers a slice loader it reaches with the bootstrap, so the slice is read once', async () => {
    let joined: Promise<void> | undefined;
    const done = bootstrap.requestBootstrap({ gapRepair: () => { joined = loadNarrow('devices'); } });
    expect(narrowReads.devices).not.toHaveBeenCalled();

    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await Promise.all([done, joined]);
    expect(narrowReads.devices).not.toHaveBeenCalled();
    expect(log).toContain('apply devices#1 d1');
  });

  it('reads narrowly again once the section has ended', async () => {
    const done = bootstrap.requestBootstrap({ gapRepair: () => {} });
    await loadNarrow('devices');
    expect(narrowReads.devices).toHaveBeenCalledTimes(1);
    reads[0].settle({ kind: 'snapshot', snapshot: DATA, etag: null });
    await done;
  });
});
