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

describe('FetchLanes.enqueue', () => {
  it('with one slot, a tie between waiting lanes goes to the lane that queued first', async () => {
    const lanes = new FetchLanes(1);
    const a = lane();
    const b = lane();
    const c = lane();
    const started: string[] = [];
    let finish!: () => void;
    const first = lanes.enqueue(a, Date.now() + 60_000, () => new Promise<void>((r) => { finish = r; }));
    const second = lanes.enqueue(b, Date.now() + 60_000, () => { started.push('B'); return Promise.resolve(); });
    const third = lanes.enqueue(c, Date.now() + 60_000, () => { started.push('C'); return Promise.resolve(); });

    finish();
    await Promise.all([first, second, third]);

    expect(started).toEqual(['B', 'C']);
  });

  it('a call whose deadline passed while it waited is rejected and never sent', async () => {
    vi.useFakeTimers();
    const lanes = new FetchLanes(1);
    const frame = lane();
    let finish!: () => void;
    const first = lanes.enqueue(frame, Date.now() + 60_000, () => new Promise<void>((r) => { finish = r; }));
    const send = vi.fn(() => Promise.resolve());
    const second = lanes.enqueue(frame, Date.now() + 50, send);
    vi.advanceTimersByTime(100);

    finish();
    await expect(second).rejects.toThrow('The app stopped waiting before the call could be sent');
    await first;
    expect(send).not.toHaveBeenCalled();
  });
});
