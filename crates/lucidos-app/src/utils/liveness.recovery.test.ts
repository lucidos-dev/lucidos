// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
vi.mock('./platform', async original => ({ ...await original<object>(), isIOSPwa: () => true }));
vi.mock('./clientLog', () => ({ postClientLog: vi.fn() }));
import { postClientLog } from './clientLog';
beforeEach(() => { vi.resetModules(); });
afterEach(() => { localStorage.clear(); vi.restoreAllMocks(); vi.clearAllMocks(); });
it('sends the persisted prior snapshot despite new samples during recovery loading', async () => {
  const { reportStartupKind } = await import('./liveness');
  const { crashStore } = await import('./crashBreadcrumbs');
  const old = crashStore(); old.add('sample', { video: 3, imagePixels: 4000000 }); old.flush();
  localStorage.setItem('lucidos-liveness-heartbeat', JSON.stringify({ ts: performance.timeOrigin - 100, focusedThread: null, draftCount: 0 }));
  const reported = reportStartupKind();
  expect(crashStore().read()).toBeNull();
  const current = crashStore(); current.add('sample', { video: 9 }); current.flush();
  await reported;
  expect(postClientLog).toHaveBeenCalledWith('lifecycle', 'startup', expect.objectContaining({ kind: 'likely_crash', breadcrumb: expect.objectContaining({ samples: expect.arrayContaining([expect.objectContaining({ data: { video: 3, imagePixels: 4000000 } })]) }) }));
  const data = vi.mocked(postClientLog).mock.calls[0][2];
  expect(new TextEncoder().encode(JSON.stringify(data)).length).toBeLessThan(4096);
  expect(crashStore().read()?.samples[0].data.video).toBe(9);
});

it.each(['shell-first', 'client-first'])('recovers the dying page when booting %s', async order => {
  const { reportStartupKind } = await import('./liveness');
  const { crashStore } = await import('./crashBreadcrumbs');
  const { startCrashMonitor } = await import('./crashMonitor');
  const { takeCrashRecord } = await import('./crashStorage');
  const old = crashStore();
  old.add('sample', { nodes: 1234, height: 5678 });
  old.flush();
  localStorage.setItem('lucidos-liveness-heartbeat', JSON.stringify({ ts: performance.timeOrigin - 100, focusedThread: null, draftCount: 0 }));
  let stop: () => void = () => {};
  try {
    if (order === 'shell-first') stop = startCrashMonitor().stop;
    const reported = reportStartupKind();
    if (order === 'client-first') stop = startCrashMonitor().stop;
    await reported;
    expect(postClientLog).toHaveBeenCalledWith('lifecycle', 'startup', expect.objectContaining({
      kind: 'likely_crash',
      breadcrumb: expect.objectContaining({ samples: [expect.objectContaining({ data: { nodes: 1234, height: 5678 } })] }),
    }));
    const current = crashStore().read();
    expect(current?.events[0].kind).toBe('thread-open');
    expect(current?.samples[0].data.nodes).not.toBe(1234);
    expect(takeCrashRecord()).toBeNull();
    expect(crashStore().read()).toEqual(current);
  } finally { stop(); }
});

it('keeps an empty boot snapshot empty after the current monitor writes', async () => {
  const { startCrashMonitor } = await import('./crashMonitor');
  const { crashStore } = await import('./crashBreadcrumbs');
  const { takeCrashRecord } = await import('./crashStorage');
  const monitor = startCrashMonitor();
  try {
    const current = crashStore().read();
    expect(current?.samples.length).toBe(1);
    expect(takeCrashRecord()).toBeNull();
    expect(takeCrashRecord()).toBeNull();
    expect(crashStore().read()).toEqual(current);
  } finally { monitor.stop(); }
});

it('stamps startup with the reporting bundle build', async () => {
  const { CLIENT_BUILD_ID } = await import('virtual:build-id');
  const { formatBuildId } = await import('./buildId');
  const { reportStartupKind } = await import('./liveness');
  await reportStartupKind();
  expect(postClientLog).toHaveBeenCalledWith('lifecycle', 'startup', expect.objectContaining({ build: formatBuildId(CLIENT_BUILD_ID) }));
});

it('records a hide without moving the last heartbeat, then clears it on wake', async () => {
  const { startLivenessTracking } = await import('./liveness');
  const stop = startLivenessTracking();
  try {
    const read = () => JSON.parse(localStorage.getItem('lucidos-liveness-heartbeat')!);
    const prior = read();
    window.dispatchEvent(new Event('pagehide'));
    expect(read()).toEqual({ ...prior, hiddenAt: expect.any(Number) });
    window.dispatchEvent(new Event('pageshow'));
    expect(read().hiddenAt).toBeUndefined();
  } finally { stop(); }
});

it('reports hidden duration without changing the crash classifier', async () => {
  const { reportStartupKind } = await import('./liveness');
  localStorage.setItem('lucidos-liveness-heartbeat', JSON.stringify({ ts: performance.timeOrigin - 200, hiddenAt: performance.timeOrigin - 100 }));
  await reportStartupKind();
  expect(postClientLog).toHaveBeenCalledWith('lifecycle', 'startup', expect.objectContaining({
    kind: 'likely_crash', dead_ms: 200, prev_hidden_ms: 100,
  }));
});
