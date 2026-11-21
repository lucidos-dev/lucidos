// @vitest-environment jsdom
import { afterEach, expect, it, vi } from 'vitest';
vi.mock('./clientLog', () => ({ postClientLog: vi.fn() }));
vi.mock('./pageHealthLog', () => ({
  pageHealthCounters: () => ({ listeners: new Map(), rafCalls: 0, intervals: 0, observers: 0 }),
}));
import { postClientLog } from './clientLog';
import { startPageHealthLog } from './pageHealthLog.report';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
  localStorage.clear();
});

it('reports visible stalls, excludes suspension and removes the reporter on teardown', () => {
  vi.useFakeTimers();
  const requestFrame = vi.spyOn(window, 'requestAnimationFrame');
  let now = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
  const stop = startPageHealthLog();
  now = 3000;
  vi.advanceTimersByTime(500);
  expect(postClientLog).toHaveBeenCalledWith('page-health', 'stall', expect.objectContaining({ lagMs: 2500 }));
  vi.mocked(postClientLog).mockClear();

  visibility.mockReturnValue('hidden');
  document.dispatchEvent(new Event('visibilitychange'));
  now = 90_000;
  vi.advanceTimersByTime(500);
  visibility.mockReturnValue('visible');
  // The browser may run a due timer before delivering the resume event.
  now += 10_000;
  vi.advanceTimersByTime(29_000);
  expect(postClientLog).not.toHaveBeenCalled();
  expect(requestFrame).not.toHaveBeenCalled();
  document.dispatchEvent(new Event('visibilitychange'));
  now += 500;
  vi.advanceTimersByTime(500);
  expect(postClientLog).not.toHaveBeenCalled();

  stop();
  now += 10_000;
  vi.advanceTimersByTime(500);
  expect(postClientLog).not.toHaveBeenCalled();
});
