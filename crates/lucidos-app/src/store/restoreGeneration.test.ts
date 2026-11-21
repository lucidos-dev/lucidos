import { describe, expect, it, vi } from 'vitest';
import { RestoreGenerationBarrier, type RestoreGeneration, type RestoreGenerationStore } from './restoreGeneration';
import { restoreGenerationDbName } from './restoreGenerationRecords';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function setup(...baseline: [] | [RestoreGeneration | undefined]) {
  let durable = baseline.length === 0 ? null : baseline[0];
  const preserved: Array<{ generation: RestoreGeneration; intent: string }> = [];
  const store: RestoreGenerationStore<string> = {
    readGeneration: vi.fn(async () => durable),
    observe: vi.fn(async (generation) => { durable = generation; }),
    preserve: vi.fn(async (generation, intent) => {
      durable = generation;
      preserved.push({ generation, intent });
    }),
  };
  const effects = { capture: vi.fn(async () => 'unsent intent'), reload: vi.fn() };
  const barrier = new RestoreGenerationBarrier(store, effects);
  return { barrier, store, effects, preserved };
}

describe('restore generation admission', () => {
  it('isolates device observations by workspace on a shared origin', () => {
    expect(restoreGenerationDbName('alpha')).not.toBe(restoreGenerationDbName('beta'));
    expect(restoreGenerationDbName(null)).toBe('lucidos-restore-generation');
  });

  it('parks startup reads until the first hello has committed', async () => {
    const { barrier, store } = setup(undefined);
    const commit = deferred<void>();
    vi.mocked(store.observe).mockReturnValue(commit.promise);
    const released = vi.fn();
    void barrier.waitUntilReady().then(released);
    const epoch = barrier.beginConnection();
    const admission = barrier.acceptHello(epoch, null);
    await vi.waitFor(() => expect(store.observe).toHaveBeenCalledWith(null, { generation: undefined }));
    expect(released).not.toHaveBeenCalled();
    expect(barrier.isReady(epoch)).toBe(false);
    commit.resolve();
    expect(await admission).toBe('ready');
    await barrier.waitUntilReady();
    expect(released).toHaveBeenCalledOnce();
  });

  it('accepts same-generation reconnects, independently of event sequence', async () => {
    const { barrier, effects } = setup('restore-a');
    const first = barrier.beginConnection();
    expect(await barrier.acceptHello(first, 'restore-a')).toBe('ready');
    const second = barrier.beginConnection();
    expect(barrier.isReady(first)).toBe(false);
    expect(barrier.isReady(second)).toBe(false);
    expect(await barrier.acceptHello(second, 'restore-a')).toBe('ready');
    expect(effects.capture).not.toHaveBeenCalled();
    expect(effects.reload).not.toHaveBeenCalled();
  });

  it('commits preserved intent before reloading and never releases stale startup', async () => {
    const { barrier, store, effects, preserved } = setup();
    const commit = deferred<void>();
    const write = vi.mocked(store.preserve).getMockImplementation()!;
    vi.mocked(store.preserve).mockImplementation(async (generation, intent) => {
      await commit.promise;
      await write(generation, intent);
    });
    const released = vi.fn();
    void barrier.waitUntilReady().then(released);
    const epoch = barrier.beginConnection();
    const admission = barrier.acceptHello(epoch, 'restore-a');
    await vi.waitFor(() => expect(store.preserve).toHaveBeenCalled());
    expect(effects.reload).not.toHaveBeenCalled();
    commit.resolve();
    expect(await admission).toBe('reload');
    expect(preserved).toEqual([{ generation: 'restore-a', intent: 'unsent intent' }]);
    expect(effects.reload).toHaveBeenCalledOnce();
    expect(released).not.toHaveBeenCalled();
    expect(barrier.isReady(epoch)).toBe(false);
    expect(await barrier.acceptHello(epoch, 'restore-a')).toBe('reload');
    expect(effects.reload).toHaveBeenCalledOnce();
  });

  it('does not reload again after the new page reads the durable generation', async () => {
    const { barrier, store, effects } = setup();
    await barrier.acceptHello(barrier.beginConnection(), 'restore-a');
    const fresh = new RestoreGenerationBarrier(store, effects);
    expect(await fresh.acceptHello(fresh.beginConnection(), 'restore-a')).toBe('ready');
    expect(effects.reload).toHaveBeenCalledOnce();
  });

  it('treats a first observed restored workspace as a boundary too', async () => {
    const { barrier, effects } = setup(undefined);
    expect(await barrier.acceptHello(barrier.beginConnection(), 'restore-a')).toBe('reload');
    expect(effects.capture).toHaveBeenCalledOnce();
  });

  it('does not adopt another tabs advanced marker into this pages baseline', async () => {
    const { barrier, store } = setup('restore-a');
    await barrier.acceptHello(barrier.beginConnection(), 'restore-a');
    await store.observe('restore-b');
    expect(await barrier.acceptHello(barrier.beginConnection(), 'restore-b')).toBe('reload');
  });

  it('fails closed on a failed capture and permits an explicit retry', async () => {
    const { barrier, effects } = setup();
    effects.capture.mockRejectedValueOnce(new Error('local write failed'));
    const epoch = barrier.beginConnection();
    await expect(barrier.acceptHello(epoch, 'restore-a')).rejects.toThrow('local write failed');
    expect(barrier.isReady(epoch)).toBe(false);
    expect(effects.reload).not.toHaveBeenCalled();
    expect(await barrier.acceptHello(epoch, 'restore-a')).toBe('reload');
  });

  it('fails closed on a failed preservation commit without marking generation observed', async () => {
    const { barrier, store, effects } = setup();
    vi.mocked(store.preserve).mockRejectedValueOnce(new Error('disk full'));
    const epoch = barrier.beginConnection();
    await expect(barrier.acceptHello(epoch, 'restore-a')).rejects.toThrow('disk full');
    expect(effects.reload).not.toHaveBeenCalled();
    expect(await store.readGeneration()).toBe(null);
    expect(await barrier.acceptHello(epoch, 'restore-a')).toBe('reload');
  });

  it('ignores closed connections both before and during capture', async () => {
    const { barrier, effects, store } = setup();
    const capture = deferred<string>();
    effects.capture.mockReturnValueOnce(capture.promise);
    const old = barrier.beginConnection();
    const pending = barrier.acceptHello(old, 'restore-a');
    await vi.waitFor(() => expect(effects.capture).toHaveBeenCalled());
    const current = barrier.beginConnection();
    capture.resolve('old intent');
    expect(await pending).toBe('stale');
    expect(store.preserve).not.toHaveBeenCalled();
    expect(await barrier.acceptHello(old, 'restore-a')).toBe('stale');
    expect(await barrier.acceptHello(current, null)).toBe('ready');
  });

  it('keeps startup parked after a disconnect while an observation commits', async () => {
    const { barrier, store } = setup();
    const commit = deferred<void>();
    vi.mocked(store.observe).mockReturnValueOnce(commit.promise);
    const old = barrier.beginConnection();
    const pending = barrier.acceptHello(old, null);
    await vi.waitFor(() => expect(store.observe).toHaveBeenCalled());
    const current = barrier.beginConnection();
    commit.resolve();
    expect(await pending).toBe('stale');
    expect(barrier.isReady(current)).toBe(false);
    expect(await barrier.acceptHello(current, null)).toBe('ready');
  });

  it('retries a failed initial device read instead of treating it as no restore', async () => {
    const { store, effects } = setup('restore-a');
    vi.mocked(store.readGeneration).mockRejectedValueOnce(new Error('database unavailable'));
    const barrier = new RestoreGenerationBarrier(store, effects);
    expect(await barrier.acceptHello(barrier.beginConnection(), 'restore-a')).toBe('ready');
    expect(effects.reload).not.toHaveBeenCalled();
    expect(barrier.baselineFromPageLoad).toBe(false);
  });

  it('proves an admission against the page-load baseline when that read worked', async () => {
    const { barrier } = setup('restore-a');
    expect(await barrier.acceptHello(barrier.beginConnection(), 'restore-a')).toBe('ready');
    expect(barrier.baselineFromPageLoad).toBe(true);
  });

  it('gives async readers a connection lease they can check before applying a response', async () => {
    const { barrier } = setup();
    const waiting = barrier.waitUntilReady();
    const first = barrier.beginConnection();
    await barrier.acceptHello(first, null);
    const lease = await waiting;
    expect(lease).toBe(first);
    expect(barrier.isReady(lease)).toBe(true);
    barrier.beginConnection();
    expect(barrier.isReady(lease)).toBe(false);
  });
});
