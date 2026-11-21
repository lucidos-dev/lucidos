// @vitest-environment jsdom
import { describe, it, expect, afterEach } from 'vitest';
import { _resetPageVisitForTesting } from './pageVisit';
import { _resetResumeClockForTesting, installResumeClock, sinceResumeMs } from './resumeClock';

function setVisibility(state: 'hidden' | 'visible') {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
  document.dispatchEvent(new Event('visibilitychange'));
}

let stop: (() => void) | null = null;
afterEach(() => {
  stop?.();
  stop = null;
  _resetResumeClockForTesting();
  _resetPageVisitForTesting();
  setVisibility('visible');
});

describe('resume clock', () => {
  it('knows no resume until the page comes back', () => {
    stop = installResumeClock();
    expect(sinceResumeMs()).toBeNull();
    setVisibility('hidden');
    expect(sinceResumeMs()).toBeNull();
  });

  it('measures from the last time the page became visible', () => {
    stop = installResumeClock();
    setVisibility('hidden');
    setVisibility('visible');
    const now = Date.now();
    expect(sinceResumeMs(now + 1500)).toBeGreaterThanOrEqual(1500);
    expect(sinceResumeMs(now + 1500)).toBeLessThan(1600);
  });

  it('counts a page restored from the back-forward cache', () => {
    stop = installResumeClock();
    window.dispatchEvent(new Event('pagehide'));
    window.dispatchEvent(Object.assign(new Event('pageshow'), { persisted: true }));
    expect(sinceResumeMs()).not.toBeNull();
  });

  it('counts a wake that brings only window focus', () => {
    stop = installResumeClock();
    window.dispatchEvent(new Event('focus'));
    expect(sinceResumeMs()).toBeNull();
    setVisibility('hidden');
    window.dispatchEvent(new Event('focus'));
    expect(sinceResumeMs()).not.toBeNull();
  });

  it('stops listening on teardown', () => {
    installResumeClock()();
    setVisibility('visible');
    expect(sinceResumeMs()).toBeNull();
  });
});
