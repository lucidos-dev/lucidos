// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { startCrashMonitor } from './crashMonitor';
import { BreadcrumbStore, crashStore } from './crashBreadcrumbs';
vi.mock('./clientLog', () => ({ postClientLog: vi.fn() }));
import { postClientLog } from './clientLog';
import { installRestoreAdmission, restoreAdmissionChanged } from './restoreAdmission';
import { trackRequest } from '../api/client/requestsInFlight';
import { focusedThreadId } from '../store/store';

afterEach(() => { focusedThreadId.value = null; vi.useRealTimers(); localStorage.clear(); document.body.innerHTML = ''; vi.clearAllMocks(); vi.restoreAllMocks(); });
describe('crash monitor', () => {
  it('records actual mounts, flushes lifecycle events and stops', async () => {
    vi.useFakeTimers();
    const monitor = startCrashMonitor();
    document.body.innerHTML = '<video></video><iframe></iframe><div class="question-body"></div>';
    await Promise.resolve();
    window.dispatchEvent(new Event('pagehide'));
    const prior = crashStore().read();
    expect(prior?.events.map(e => e.kind)).toEqual(expect.arrayContaining(['thread-open', 'media-mount', 'iframe-mount', 'answer-card-render']));
    expect(prior?.events.find(e => e.kind === 'thread-open')?.data).toEqual({});
    expect(prior?.samples[prior.samples.length - 1]?.data.video).toBe(1);
    monitor.stop();
    const saved = JSON.stringify(crashStore().read());
    document.body.innerHTML = '';
    await Promise.resolve();
    vi.advanceTimersByTime(30_000);
    window.dispatchEvent(new Event('pagehide'));
    expect(JSON.stringify(crashStore().read())).toBe(saved);
  });
  it('bounds mount inspection inside a single large subtree', async () => {
    const monitor = startCrashMonitor();
    const wrapper = document.createElement('div');
    wrapper.innerHTML = '<img>'.repeat(1000);
    const matches = vi.spyOn(Element.prototype, 'matches');
    document.body.append(wrapper);
    await Promise.resolve();
    expect(matches.mock.calls.length).toBeLessThanOrEqual(768);
    window.dispatchEvent(new Event('pagehide'));
    const mounts = crashStore().read()?.events.filter(e => e.kind === 'media-mount');
    expect(mounts?.[0].data.count).toBeLessThan(1000);
    monitor.stop();
  });
  it('samples densely for twenty seconds, returns to ten seconds and restarts on focus', () => {
    vi.useFakeTimers();
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const monitor = startCrashMonitor();
    const start = Date.now();
    const latest = () => crashStore().read()?.samples.slice(-1)[0]?.at;
    vi.advanceTimersByTime(1999);
    expect(latest()).toBe(start);
    vi.advanceTimersByTime(1);
    expect(latest()).toBe(start + 2000);
    vi.advanceTimersByTime(18_000);
    expect(latest()).toBe(start + 20_000);
    vi.advanceTimersByTime(9999);
    expect(latest()).toBe(start + 20_000);
    vi.advanceTimersByTime(1);
    expect(latest()).toBe(start + 30_000);
    focusedThreadId.value = 'another-thread';
    vi.advanceTimersByTime(2000);
    expect(latest()).toBe(start + 32_000);
    monitor.stop();
  });
  it('persists only while visible and flushes when hidden', () => {
    vi.useFakeTimers();
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const monitor = startCrashMonitor();
    document.body.innerHTML = '<video></video>';
    vi.advanceTimersByTime(1999);
    expect(crashStore().read()?.samples).toHaveLength(1);
    vi.advanceTimersByTime(1);
    expect(crashStore().read()?.samples).toHaveLength(2);
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    const saved = JSON.stringify(crashStore().read());
    vi.advanceTimersByTime(30_000);
    expect(JSON.stringify(crashStore().read())).toBe(saved);
    monitor.stop();
  });
  it.each(['visibility', 'bfcache', 'visibility-first', 'pageshow-first'])('restarts dense sampling and flushes once after %s resume', source => {
    vi.useFakeTimers();
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const monitor = startCrashMonitor();
    try {
      vi.advanceTimersByTime(30_000);
      visibility.mockReturnValue('hidden');
      document.dispatchEvent(new Event('visibilitychange'));
      vi.advanceTimersByTime(77_000);
      visibility.mockReturnValue('visible');
      const resumedAt = Date.now();
      if (source === 'visibility' || source === 'visibility-first') document.dispatchEvent(new Event('visibilitychange'));
      else window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
      if (source === 'visibility-first') {
        window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
      } else if (source === 'pageshow-first') {
        document.dispatchEvent(new Event('visibilitychange'));
      }
      const latest = () => crashStore().read()?.samples.slice(-1)[0]?.at;
      expect(latest()).toBe(resumedAt);
      expect(crashStore().read()?.samples.filter(sample => sample.at === resumedAt)).toHaveLength(1);
      vi.advanceTimersByTime(499);
      expect(latest()).toBe(resumedAt);
      vi.advanceTimersByTime(1);
      expect(latest()).toBe(resumedAt + 500);
      vi.advanceTimersByTime(14_500);
      expect(latest()).toBe(resumedAt + 15_000);
      vi.advanceTimersByTime(9999);
      expect(latest()).toBe(resumedAt + 15_000);
      vi.advanceTimersByTime(1);
      expect(latest()).toBe(resumedAt + 25_000);
      monitor.stop();
      const saved = JSON.stringify(crashStore().read());
      window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
      document.dispatchEvent(new Event('visibilitychange'));
      vi.advanceTimersByTime(2000);
      expect(JSON.stringify(crashStore().read())).toBe(saved);
    } finally { monitor.stop(); }
  });
  it('coalesces first transcript render and resource mounts during the dense window', async () => {
    vi.useFakeTimers();
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    focusedThreadId.value = 'sample-thread';
    const monitor = startCrashMonitor();
    document.body.innerHTML = '<div class="thread-content"><div class="chat-exchange" data-thread-id="sample-thread"><div data-event-id="sample-event"></div></div></div>';
    await Promise.resolve();
    vi.advanceTimersByTime(250);
    expect(crashStore().read()?.samples.slice(-1)[0]?.data.events).toBe(1);
    expect(crashStore().read()?.samples).toHaveLength(2);
    document.body.insertAdjacentHTML('beforeend', '<video></video><audio></audio><iframe></iframe><img>');
    Object.defineProperties(document.querySelector('img')!, { naturalWidth: { value: 1024 }, naturalHeight: { value: 1024 } });
    await Promise.resolve();
    vi.advanceTimersByTime(249);
    expect(crashStore().read()?.samples).toHaveLength(2);
    vi.advanceTimersByTime(1);
    expect(crashStore().read()?.samples).toHaveLength(3);
    expect(crashStore().read()?.samples.slice(-1)[0]?.data).toMatchObject({ video: 1, audio: 1, iframes: 1, imagePixels: 1048576 });
    monitor.stop();
  });
  it.each(['video', 'audio', 'iframe', 'img'])('captures an isolated %s mount before the periodic sample', async tag => {
    vi.useFakeTimers();
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const monitor = startCrashMonitor();
    const element = document.createElement(tag);
    if (tag === 'img') Object.defineProperties(element, { naturalWidth: { value: 1024 }, naturalHeight: { value: 1024 } });
    document.body.append(element);
    await Promise.resolve();
    vi.advanceTimersByTime(250);
    expect(crashStore().read()?.samples).toHaveLength(2);
    monitor.stop();
  });
  it('samples a large image on load, skips icons and stops event sampling after the window', async () => {
    vi.useFakeTimers();
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const monitor = startCrashMonitor();
    const image = document.createElement('img');
    document.body.append(image);
    await Promise.resolve();
    vi.advanceTimersByTime(250);
    expect(crashStore().read()?.samples).toHaveLength(1);
    Object.defineProperties(image, { naturalWidth: { value: 1024 }, naturalHeight: { value: 1024 } });
    image.dispatchEvent(new Event('load'));
    vi.advanceTimersByTime(250);
    expect(crashStore().read()?.samples.slice(-1)[0]?.data.imagePixels).toBe(1048576);
    vi.advanceTimersByTime(20_000);
    const saved = JSON.stringify(crashStore().read()?.samples);
    document.body.append(document.createElement('video'));
    image.dispatchEvent(new Event('load'));
    await Promise.resolve();
    vi.advanceTimersByTime(250);
    expect(JSON.stringify(crashStore().read()?.samples)).toBe(saved);
    monitor.stop();
  });
  it('cancels queued samples on focus change and teardown', async () => {
    vi.useFakeTimers();
    vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
    vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
    const monitor = startCrashMonitor();
    document.body.append(document.createElement('video'));
    await Promise.resolve();
    focusedThreadId.value = 'another-thread';
    const saved = JSON.stringify(crashStore().read());
    vi.advanceTimersByTime(250);
    expect(JSON.stringify(crashStore().read())).toBe(saved);
    document.body.append(document.createElement('iframe'));
    await Promise.resolve();
    monitor.stop();
    const stopped = JSON.stringify(crashStore().read());
    vi.advanceTimersByTime(30_000);
    expect(JSON.stringify(crashStore().read())).toBe(stopped);
  });
  it('throttles recovered stalls', () => {
    const monitor = startCrashMonitor();
    monitor.stall(100);
    monitor.stall(2500);
    monitor.stall(3000);
    expect(postClientLog).toHaveBeenCalledTimes(1);
    expect(postClientLog).toHaveBeenCalledWith('page-health', 'stall', expect.objectContaining({ lagMs: 2500 }));
    monitor.stop();
  });
});

it('persists observed resume work immediately and records request counts without paths', async () => {
  vi.useFakeTimers();
  vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const monitor = startCrashMonitor();
  const finish = trackRequest('/private/thread-id');
  try {
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    visibility.mockReturnValue('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    let phase = 'awaiting-hello' as 'awaiting-hello' | 'journal' | 'admitted';
    installRestoreAdmission({ wait: async () => {}, ready: () => phase === 'admitted', phase: () => phase });
    phase = 'journal';
    document.documentElement.dataset.lucidosEventStream = 'connecting';
    vi.advanceTimersByTime(500);
    await Promise.resolve();
    expect(crashStore().read()?.events).toEqual(expect.arrayContaining([
      expect.objectContaining({ kind: 'wake' }),
      expect.objectContaining({ kind: 'admission', data: { phase: 4 } }),
      expect.objectContaining({ kind: 'stream', data: { state: 1 } }),
    ]));
    expect(crashStore().read()?.samples.slice(-1)[0].data).toMatchObject({ inflight: 1, oldestMs: 500 });
    expect(JSON.stringify(crashStore().read())).not.toContain('/private');
    phase = 'admitted';
    restoreAdmissionChanged();
    expect(crashStore().read()?.events.slice(-1)[0]).toMatchObject({ kind: 'admission', data: { phase: 7 } });
    document.body.innerHTML = '<div class="thread-content"><div class="chat-exchange" data-thread-id="focused"></div></div><iframe></iframe>';
    focusedThreadId.value = 'focused';
    await Promise.resolve();
    document.querySelector('iframe')!.dispatchEvent(new Event('load'));
    expect(crashStore().read()?.events.map(e => e.kind)).toEqual(expect.arrayContaining(['transcript', 'iframe-load']));
  } finally { finish(); monitor.stop(); }
});

it('bounds fine sampling to thirty visible ticks per wake and measures collection cost', () => {
  vi.useFakeTimers();
  const clock = vi.spyOn(performance, 'now').mockImplementation(() => Date.now());
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const add = vi.spyOn(BreadcrumbStore.prototype, 'add');
  const monitor = startCrashMonitor();
  try {
    vi.advanceTimersByTime(30_000);
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    visibility.mockReturnValue('visible');
    document.dispatchEvent(new Event('visibilitychange'));
    add.mockClear();
    vi.advanceTimersByTime(15_000);
    expect(add.mock.calls.filter(([kind]) => kind === 'sample')).toHaveLength(30);
    vi.advanceTimersByTime(9999);
    expect(add.mock.calls.filter(([kind]) => kind === 'sample')).toHaveLength(30);
    clock.mockReturnValueOnce(Date.now()).mockReturnValueOnce(Date.now() + 3);
    vi.advanceTimersByTime(1);
    expect(add.mock.calls.filter(([kind]) => kind === 'sample').slice(-1)[0][1].costMs).toBe(3);
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
    add.mockClear();
    vi.advanceTimersByTime(30_000);
    expect(add.mock.calls.filter(([kind]) => kind === 'sample')).toHaveLength(0);
  } finally { monitor.stop(); }
});

it.each([false, true])('records the existing pagehide listener state (persisted: %s)', async persisted => {
  const { startLivenessTracking } = await import('./liveness');
  const stop = startLivenessTracking();
  const monitor = startCrashMonitor();
  try {
    window.dispatchEvent(new PageTransitionEvent('pagehide', { persisted }));
    expect(crashStore().read()?.events.filter(e => e.kind === 'hide').slice(-1)[0]?.data)
      .toEqual({ state: persisted ? 2 : 1, count: 0 });
  } finally { monitor.stop(); stop(); }
});
