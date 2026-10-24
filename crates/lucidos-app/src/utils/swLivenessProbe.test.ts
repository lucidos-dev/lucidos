import { describe, it, expect, vi, beforeEach, afterEach, type Mock } from 'vitest';
import { createSwLivenessProbe, SW_PROBE_TIMEOUT_MS, SW_RECOVERY_COOLDOWN_MS } from './swLivenessProbe';

/** The probe exists for Chrome's wedged service worker, which answers no
 *  message at all. Its repair unregisters the worker and resubscribes push, and
 *  it toasts "Notifications repaired". So a probe that misreads a healthy worker
 *  costs the user a push endpoint and a toast that makes no sense. An iOS PWA
 *  hit exactly that on resume: the page froze mid-probe, and on wake the
 *  overdue timeout beat the queued pong. */

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  document.dispatchEvent(new Event('visibilitychange'));
}

let pings = 0;
let recover: Mock<() => Promise<void>>;
let onRecovered: Mock<() => void>;

function probe(opts: { controller?: boolean } = {}) {
  return createSwLivenessProbe({
    ping: () => {
      if (opts.controller === false) return false;
      pings += 1;
      return true;
    },
    recover,
    onRecovered,
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  pings = 0;
  recover = vi.fn(() => Promise.resolve());
  onRecovered = vi.fn(() => {});
});

afterEach(() => {
  vi.useRealTimers();
  setVisibility('visible');
});

describe('createSwLivenessProbe', () => {
  it('leaves a worker that answers alone', async () => {
    const p = probe();
    const done = p.check();
    p.notePong();
    await done;
    expect(pings).toBe(1);
    expect(recover).not.toHaveBeenCalled();
  });

  it('repairs a worker that stays silent through two probes', async () => {
    const p = probe();
    const done = p.check();
    await vi.advanceTimersByTimeAsync(2 * SW_PROBE_TIMEOUT_MS);
    await done;
    expect(pings).toBe(2);
    expect(recover).toHaveBeenCalledTimes(1);
    expect(onRecovered).toHaveBeenCalledTimes(1);
  });

  it('spares a worker that misses the first ping and answers the second', async () => {
    // A worker cold-starting after an iOS wake can miss one window.
    const p = probe();
    const done = p.check();
    await vi.advanceTimersByTimeAsync(SW_PROBE_TIMEOUT_MS);
    expect(pings).toBe(2);
    p.notePong();
    await done;
    expect(recover).not.toHaveBeenCalled();
    expect(onRecovered).not.toHaveBeenCalled();
  });

  it('reads a probe the page was hidden for as no evidence', async () => {
    // The iOS freeze: hidden mid-probe, timer overdue on wake, pong still queued.
    const p = probe();
    const done = p.check();
    setVisibility('hidden');
    setVisibility('visible');
    await vi.advanceTimersByTimeAsync(2 * SW_PROBE_TIMEOUT_MS);
    await done;
    expect(pings).toBe(1);
    expect(recover).not.toHaveBeenCalled();
  });

  it('reads a hide during the confirming probe as no evidence too', async () => {
    const p = probe();
    const done = p.check();
    await vi.advanceTimersByTimeAsync(SW_PROBE_TIMEOUT_MS);
    setVisibility('hidden');
    setVisibility('visible');
    await vi.advanceTimersByTimeAsync(SW_PROBE_TIMEOUT_MS);
    await done;
    expect(pings).toBe(2);
    expect(recover).not.toHaveBeenCalled();
  });

  it('does nothing without a controller', async () => {
    const p = probe({ controller: false });
    await p.check();
    expect(recover).not.toHaveBeenCalled();
  });

  it('waits out the cooldown before probing again after a repair', async () => {
    const p = probe();
    const first = p.check();
    await vi.advanceTimersByTimeAsync(2 * SW_PROBE_TIMEOUT_MS);
    await first;
    expect(pings).toBe(2);
    await p.check();
    expect(pings).toBe(2);
    vi.advanceTimersByTime(SW_RECOVERY_COOLDOWN_MS);
    const later = p.check();
    p.notePong();
    await later;
    expect(pings).toBe(3);
  });

  it('runs one probe at a time', async () => {
    const p = probe();
    const first = p.check();
    await p.check();
    p.notePong();
    await first;
    expect(pings).toBe(1);
  });

  it('neither repairs nor toasts once stopped', async () => {
    const p = probe();
    const done = p.check();
    p.stop();
    await vi.advanceTimersByTimeAsync(2 * SW_PROBE_TIMEOUT_MS);
    await done;
    expect(recover).not.toHaveBeenCalled();
    expect(onRecovered).not.toHaveBeenCalled();
  });
});
