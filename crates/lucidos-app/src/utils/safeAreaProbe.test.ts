// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('./clientLog', () => ({ postClientLog: vi.fn() }));

import { postClientLog } from './clientLog';
import { reportSafeAreaReading, resetSafeAreaProbe, scheduleSafeAreaSnapshot } from './safeAreaProbe';

const PORTRAIT = { top: 59, right: 0, bottom: 34, left: 0 };
const TOP_LOST = { top: 0, right: 0, bottom: 34, left: 0 };

describe('reportSafeAreaReading', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetSafeAreaProbe();
    vi.mocked(postClientLog).mockClear();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('logs the reading, the floor and the header geometry', () => {
    const header = document.createElement('header');
    header.className = 'app-header';
    header.appendChild(document.createElement('button'));
    document.body.appendChild(header);
    reportSafeAreaReading(TOP_LOST, PORTRAIT);
    expect(postClientLog).toHaveBeenCalledWith('safe-area', 'reading', expect.objectContaining({
      env: TOP_LOST, floor: PORTRAIT, header: [0, 0], firstButton: [0, 0],
    }));
    header.remove();
  });

  it('logs only when the state changes', () => {
    reportSafeAreaReading(PORTRAIT, null);
    reportSafeAreaReading(PORTRAIT, null);
    reportSafeAreaReading(TOP_LOST, PORTRAIT);
    expect(postClientLog).toHaveBeenCalledTimes(2);
  });

  it('logs the latest reading again once the header has mounted', () => {
    reportSafeAreaReading(PORTRAIT, null);
    scheduleSafeAreaSnapshot();
    vi.advanceTimersByTime(3000);
    expect(postClientLog).toHaveBeenCalledTimes(1);
    const header = document.createElement('header');
    header.className = 'app-header';
    document.body.appendChild(header);
    vi.advanceTimersByTime(3000);
    expect(postClientLog).toHaveBeenLastCalledWith('safe-area', 'settled', expect.objectContaining({
      env: PORTRAIT, header: [0, 0],
    }));
    header.remove();
  });

  it('logs the snapshot without a header once it stops waiting', () => {
    reportSafeAreaReading(PORTRAIT, null);
    scheduleSafeAreaSnapshot();
    vi.runAllTimers();
    expect(postClientLog).toHaveBeenLastCalledWith('safe-area', 'settled', expect.objectContaining({ header: null }));
  });
});
