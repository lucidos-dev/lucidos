// @vitest-environment jsdom
/**
 * One interval for every subscriber, and none once the last one leaves.
 *
 * The value it hands out has to keep moving, because a frozen one is the exact
 * bug this hook exists to kill: an eight-hour outage reading "for 2 minutes".
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { COARSE_TICK_MS, useCoarseClock } from '../useCoarseClock';

let seen: number[] = [];

function Probe() {
  seen.push(useCoarseClock());
  return null;
}

/** What the last render of a `Probe` read. */
function latest(): number {
  return seen[seen.length - 1];
}

describe('useCoarseClock', () => {
  let host: HTMLElement;

  beforeEach(() => {
    seen = [];
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-08-27T06:00:00Z'));
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    vi.useRealTimers();
  });

  it('serves every subscriber from one interval', () => {
    const started = vi.spyOn(globalThis, 'setInterval');
    render(<div><Probe /><Probe /><Probe /></div>, host);
    expect(started).toHaveBeenCalledTimes(1);
    expect(started).toHaveBeenCalledWith(expect.any(Function), COARSE_TICK_MS);
    started.mockRestore();
  });

  it('stops the interval only when the last subscriber unmounts', () => {
    const stopped = vi.spyOn(globalThis, 'clearInterval');
    render(<div><Probe /><Probe /></div>, host);
    render(<div><Probe /></div>, host);
    expect(stopped).not.toHaveBeenCalled();

    render(null, host);
    expect(stopped).toHaveBeenCalledTimes(1);
    stopped.mockRestore();
  });

  it('advances the value it hands out', () => {
    render(<Probe />, host);
    const first = latest();

    vi.advanceTimersByTime(COARSE_TICK_MS);
    render(<Probe />, host);

    expect(latest()).toBe(first + COARSE_TICK_MS);
  });

  it('catches the value up when a subscriber returns after a gap', () => {
    render(<Probe />, host);
    render(null, host);

    // Nothing ticked while nobody was reading, so the value is eight hours old.
    vi.setSystemTime(new Date('2026-08-27T14:00:00Z'));
    render(<Probe />, host);
    render(<Probe />, host);

    expect(latest()).toBe(Date.now());
  });
});
