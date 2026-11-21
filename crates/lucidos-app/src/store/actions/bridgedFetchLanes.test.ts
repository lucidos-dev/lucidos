import { describe, it, expect, vi, afterEach } from 'vitest';
import { FetchLanes } from './bridgedFetchLanes';

const lane = (): { isConnected: boolean } => ({ isConnected: true });

afterEach(() => vi.useRealTimers());

describe('FetchLanes.snapshot', () => {
  it('is empty at rest', () => {
    expect(new FetchLanes(2).snapshot()).toEqual({
      running: 0, queued: 0, oldestRunningMs: null, oldestQueuedMs: null,
    });
  });

  it('counts running and queued calls with the age of the oldest of each', async () => {
    vi.useFakeTimers();
    const lanes = new FetchLanes(1);
    const frame = lane();
    let finish!: () => void;
    const first = lanes.enqueue(frame, Date.now() + 60_000, () => new Promise<void>((r) => { finish = r; }));
    vi.advanceTimersByTime(500);
    const second = lanes.enqueue(frame, Date.now() + 60_000, () => Promise.resolve());
    vi.advanceTimersByTime(250);

    expect(lanes.snapshot()).toEqual({ running: 1, queued: 1, oldestRunningMs: 750, oldestQueuedMs: 250 });

    finish();
    await Promise.all([first, second]);
    await vi.advanceTimersByTimeAsync(0);
    expect(lanes.snapshot()).toEqual({ running: 0, queued: 0, oldestRunningMs: null, oldestQueuedMs: null });
  });
});
