import { describe, expect, it, vi } from 'vitest';
import type { BootstrapSliceKey } from '../api/client/bootstrap';
import { joinSliceLoadsDuring, loadSlice, sharedSliceLoader, sliceLoadStarts, startSliceLoad, type BootstrapSlice } from './bootstrapSliceLoads';

function recordingSlice(log: string[]): BootstrapSlice<string, number> {
  let begun = 0;
  return {
    begin: () => { log.push('begin'); return ++begun; },
    apply: (body, ctx) => { log.push(`apply ${ctx} ${body}`); },
    fail: (error, ctx) => { log.push(`fail ${ctx} ${(error as Error).message}`); },
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

/** The helper never reads a body, so a string stands in for each real one. */
function load(key: BootstrapSliceKey, slice: BootstrapSlice<string, number>, read: () => Promise<string>): Promise<void> {
  return loadSlice(key, slice as never, read as never);
}

describe('loadSlice', () => {
  it('begins, reads, then applies the body with the context begin returned', async () => {
    const log: string[] = [];
    await load('users', recordingSlice(log), () => Promise.resolve('body'));
    expect(log).toEqual(['begin', 'apply 1 body']);
  });

  it('hands a failed read to fail, and never throws', async () => {
    const log: string[] = [];
    await load('users', recordingSlice(log), () => Promise.reject(new Error('down')));
    expect(log).toEqual(['begin', 'fail 1 down']);
  });

  it('hands an apply that throws to fail, as the loaders it replaces did', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    slice.apply = () => { throw new Error('bad body'); };
    await load('users', slice, () => Promise.resolve('body'));
    expect(log).toEqual(['begin', 'fail 1 bad body']);
  });

  it('still applies an older answer after a newer one could not be applied', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const apply = slice.apply;
    slice.apply = (body, ctx) => {
      if (body === 'bad') throw new Error('bad body');
      return apply(body, ctx);
    };
    const older = deferred<string>();
    const first = load('users', slice, () => older.promise);
    await load('users', slice, () => Promise.resolve('bad'));
    older.resolve('good');
    await first;
    expect(log).toEqual(['begin', 'begin', 'fail 2 bad body', 'apply 1 good']);
  });

  it('applies only the newer of two overlapping reads, when the older lands last', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const older = deferred<string>();
    const first = load('users', slice, () => older.promise);
    await load('users', slice, () => Promise.resolve('new'));
    older.resolve('old');
    await first;
    expect(log).toEqual(['begin', 'begin', 'apply 2 new']);
  });

  it('never fails the slice for an older read that rejects after a newer one applied', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const older = deferred<string>();
    const first = load('users', slice, () => older.promise);
    await load('users', slice, () => Promise.resolve('new'));
    older.reject(new Error('down'));
    await first;
    expect(log).toEqual(['begin', 'begin', 'apply 2 new']);
  });

  it('applies an older read that lands first, then the newer one', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const older = deferred<string>();
    const newer = deferred<string>();
    const first = load('users', slice, () => older.promise);
    const second = load('users', slice, () => newer.promise);
    older.resolve('old');
    await first;
    newer.resolve('new');
    await second;
    expect(log).toEqual(['begin', 'begin', 'apply 1 old', 'apply 2 new']);
  });

  it('still applies reads that each land after the next one started', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const reads = [deferred<string>(), deferred<string>(), deferred<string>()];
    const loads = reads.map((read) => load('users', slice, () => read.promise));
    reads[0].resolve('a');
    await loads[0];
    reads[1].resolve('b');
    await loads[1];
    expect(log).toEqual(['begin', 'begin', 'begin', 'apply 1 a', 'apply 2 b']);
    reads[2].resolve('c');
    await loads[2];
  });

  it('applies an older answer that lands after the newest read failed', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const older = deferred<string>();
    const first = load('users', slice, () => older.promise);
    await load('users', slice, () => Promise.reject(new Error('down')));
    older.resolve('old');
    await first;
    expect(log).toEqual(['begin', 'begin', 'fail 2 down', 'apply 1 old']);
  });

  it('leaves an older failure to the newer read still out, so every context ends once', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    slice.abandon = (ctx) => { log.push(`abandon ${ctx}`); };
    const older = deferred<string>();
    const newer = deferred<string>();
    const first = load('users', slice, () => older.promise);
    const second = load('users', slice, () => newer.promise);
    older.reject(new Error('down'));
    await first;
    newer.resolve('new');
    await second;
    expect(log).toEqual(['begin', 'begin', 'abandon 1', 'apply 2 new']);
  });

  it('leaves an older answer it could not apply to the newer read still out', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    slice.abandon = (ctx) => { log.push(`abandon ${ctx}`); };
    const apply = slice.apply;
    slice.apply = (body, ctx) => {
      if (body === 'bad') throw new Error('bad body');
      return apply(body, ctx);
    };
    const older = deferred<string>();
    const newer = deferred<string>();
    const first = load('users', slice, () => older.promise);
    const second = load('users', slice, () => newer.promise);
    older.resolve('bad');
    await first;
    newer.resolve('new');
    await second;
    expect(log).toEqual(['begin', 'begin', 'abandon 1', 'apply 2 new']);
  });

  it('counts one start per load, per slice', async () => {
    const before = sliceLoadStarts('devices');
    const other = sliceLoadStarts('triggers');
    await load('devices', recordingSlice([]), () => Promise.resolve('body'));
    expect(sliceLoadStarts('devices')).toBe(before + 1);
    expect(sliceLoadStarts('triggers')).toBe(other);
  });
});

describe('sharedSliceLoader', () => {
  it('shares one read between concurrent callers', async () => {
    const log: string[] = [];
    const read = deferred<string>();
    const loader = sharedSliceLoader('users', recordingSlice(log) as never, () => read.promise as never);
    const first = loader.load();
    expect(loader.load()).toBe(first);
    read.resolve('body');
    await first;
    expect(log).toEqual(['begin', 'apply 1 body']);
  });

  it('frees the shared slot before the read settles, so no caller joins a settled read', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const read = deferred<string>();
    const loader = sharedSliceLoader('users', slice as never, () => read.promise as never);
    slice.fail = () => { log.push(`fail, shared ${loader.inFlight() === null ? 'free' : 'held'}`); };
    const first = loader.load();
    read.reject(new Error('down'));
    await first;
    expect(log).toEqual(['begin', 'fail, shared free']);
    expect(loader.load()).not.toBe(first);
  });

  it('shares nothing from a read that threw before its promise', async () => {
    const log: string[] = [];
    const loader = sharedSliceLoader('users', recordingSlice(log) as never, () => { throw new Error('no client'); });
    const first = loader.load();
    expect(loader.inFlight()).toBeNull();
    await first;
    expect(log).toEqual(['begin', 'fail 1 no client']);
  });

  it('frees the shared slot when begin throws, so the next caller reads again', async () => {
    const log: string[] = [];
    const slice = recordingSlice(log);
    const begin = slice.begin;
    slice.begin = () => { throw new Error('begin'); };
    const loader = sharedSliceLoader('users', slice as never, () => Promise.resolve('body') as never);
    await expect(loader.load()).rejects.toThrow('begin');
    expect(loader.inFlight()).toBeNull();
    slice.begin = begin;
    await loader.load();
    expect(log).toEqual(['begin', 'apply 1 body']);
  });
});

describe('joinSliceLoadsDuring', () => {
  it('answers a loader called inside the section with the bootstrap, and reads nothing', async () => {
    const bootstrap = Promise.resolve();
    const read = vi.fn(() => Promise.resolve('body'));
    const before = sliceLoadStarts('apps');
    let joined: Promise<void> | undefined;
    joinSliceLoadsDuring(bootstrap, () => { joined = load('apps', recordingSlice([]), read); });
    await joined;
    expect(read).not.toHaveBeenCalled();
    expect(sliceLoadStarts('apps')).toBe(before);
    expect(startSliceLoad('apps')).toBeNull();
  });

  it('stops joining when the section throws', () => {
    expect(() => joinSliceLoadsDuring(Promise.resolve(), () => { throw new Error('boom'); })).toThrow('boom');
    expect(startSliceLoad('apps')).toBeNull();
  });
});
