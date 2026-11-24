import { describe, it, expect } from 'vitest';
import { runWithConcurrency } from './concurrentPool';

function deferred(): { promise: Promise<void>; resolve: () => void; reject: (e: unknown) => void } {
  let resolve!: () => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

describe('runWithConcurrency', () => {
  it('never exceeds the limit, and keeps it saturated while work remains', async () => {
    const gates = Array.from({ length: 10 }, deferred);
    let inFlight = 0;
    let peak = 0;
    const started: number[] = [];

    const done = runWithConcurrency(gates.map((_, i) => i), 3, async (i) => {
      started.push(i);
      inFlight++;
      peak = Math.max(peak, inFlight);
      await gates[i].promise;
      inFlight--;
    });

    await Promise.resolve();
    expect(started).toEqual([0, 1, 2]);

    for (const gate of gates) {
      gate.resolve();
      await Promise.resolve();
      await Promise.resolve();
    }
    await done;

    expect(peak).toBe(3);
    expect(started).toHaveLength(10);
  });

  it('runs every item and resolves only after the last one settles', async () => {
    const last = deferred();
    const ran: number[] = [];
    let settled = false;

    const done = runWithConcurrency([0, 1, 2, 3], 2, async (i) => {
      ran.push(i);
      if (i === 3) await last.promise;
    }).then(() => { settled = true; });

    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(settled).toBe(false);

    last.resolve();
    await done;
    expect(settled).toBe(true);
    expect(ran.sort()).toEqual([0, 1, 2, 3]);
  });

  it('a throwing task neither rejects the pool nor cancels its siblings', async () => {
    const ran: number[] = [];

    await runWithConcurrency([0, 1, 2, 3, 4], 2, async (i) => {
      ran.push(i);
      if (i === 1) throw new Error('boom');
    });

    expect(ran.sort()).toEqual([0, 1, 2, 3, 4]);
  });

  it('a limit above the item count starts everything at once', async () => {
    const gates = Array.from({ length: 3 }, deferred);
    let inFlight = 0;
    let peak = 0;

    const done = runWithConcurrency([0, 1, 2], 10, async (i) => {
      inFlight++;
      peak = Math.max(peak, inFlight);
      await gates[i].promise;
      inFlight--;
    });

    await Promise.resolve();
    expect(peak).toBe(3);
    gates.forEach(g => g.resolve());
    await done;
  });

  it('is a no-op on an empty list', async () => {
    let called = false;
    await runWithConcurrency([], 4, async () => { called = true; });
    expect(called).toBe(false);
  });

  it('rejects a non-positive limit rather than silently running unbounded', async () => {
    await expect(runWithConcurrency([1], 0, async () => {})).rejects.toThrow('limit must be > 0');
  });
});
