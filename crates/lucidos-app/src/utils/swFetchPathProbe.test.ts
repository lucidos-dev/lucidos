import { describe, it, expect, vi, beforeEach, afterEach, type Mock } from 'vitest';
import {
  createSwFetchPathProbe,
  claimFetchPathRecovery,
  fetchProbeUrls,
  FETCH_PROBE_PARAM,
  SW_FETCH_PROBE_TIMEOUT_MS,
  SW_FETCH_RECOVERY_WINDOW_MS,
} from './swFetchPathProbe';
import { _resetPageVisitForTesting } from './pageVisit';
import { BOOT_SPLASH_SAFETY_MS } from '../hooks/useBootSplashReady';

const HEALTH = '/api/v1/health';
const NEVER = new Promise<never>(() => {});

let recover: Mock<() => Promise<void>>;
let onSuspected: Mock<() => void>;
let onWedged: Mock<() => void>;

function setVisibility(state: DocumentVisibilityState) {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state });
  document.dispatchEvent(new Event('visibilitychange'));
}

type Reply = 'answers' | 'hangs' | 'fails';

function probe(opts: { direct: Reply; forwarded: Reply; controller?: boolean }) {
  const reply = (r: Reply) => (r === 'answers' ? Promise.resolve(new Response('ok')) : r === 'fails' ? Promise.reject(new Error('net')) : NEVER);
  return createSwFetchPathProbe({
    hasController: () => opts.controller !== false,
    fetchUrl: (url) => reply(url.includes(FETCH_PROBE_PARAM) ? opts.forwarded : opts.direct),
    healthUrl: HEALTH,
    recover,
    onSuspected,
    onWedged,
  });
}

async function runToDeadline(p: ReturnType<typeof probe>) {
  const done = p.check();
  await vi.advanceTimersByTimeAsync(2 * SW_FETCH_PROBE_TIMEOUT_MS);
  await done;
}

beforeEach(() => {
  vi.useFakeTimers();
  sessionStorage.clear();
  recover = vi.fn(() => Promise.resolve());
  onSuspected = vi.fn(() => {});
  onWedged = vi.fn(() => {});
});

afterEach(() => {
  vi.useRealTimers();
  setVisibility('visible');
  _resetPageVisitForTesting();
});

describe('createSwFetchPathProbe', () => {
  it('leaves a worker whose forwarded probe answers alone', async () => {
    await runToDeadline(probe({ direct: 'answers', forwarded: 'answers' }));
    expect(recover).not.toHaveBeenCalled();
    expect(onSuspected).not.toHaveBeenCalled();
    expect(onWedged).not.toHaveBeenCalled();
  });

  it('recovers before the boot splash gives up, so a wedged boot never strands on it', async () => {
    const p = probe({ direct: 'answers', forwarded: 'hangs' });
    const done = p.check();
    await vi.advanceTimersByTimeAsync(BOOT_SPLASH_SAFETY_MS - 1);
    expect(recover).toHaveBeenCalledTimes(1);
    await done;
  });

  it('reads a forwarded probe that fails fast as no hang', async () => {
    await runToDeadline(probe({ direct: 'answers', forwarded: 'fails' }));
    expect(recover).not.toHaveBeenCalled();
  });

  it('never reads a network outage as a wedge', async () => {
    await runToDeadline(probe({ direct: 'fails', forwarded: 'hangs' }));
    expect(recover).not.toHaveBeenCalled();
  });

  it('gives no verdict when both paths hang', async () => {
    await runToDeadline(probe({ direct: 'hangs', forwarded: 'hangs' }));
    expect(recover).not.toHaveBeenCalled();
  });

  it('skips the probe when no worker controls the page', async () => {
    const fetchUrl = vi.fn(() => NEVER);
    const p = createSwFetchPathProbe({ hasController: () => false, fetchUrl, healthUrl: HEALTH, recover, onSuspected, onWedged });
    await p.check();
    expect(fetchUrl).not.toHaveBeenCalled();
  });

  it('recovers once when the forwarded probe hangs and the plain one answers', async () => {
    await runToDeadline(probe({ direct: 'answers', forwarded: 'hangs' }));
    expect(onWedged).toHaveBeenCalledTimes(1);
    expect(recover).toHaveBeenCalledTimes(1);
  });

  it('leaves a worker that misses one window and answers the next alone', async () => {
    let forwardedCalls = 0;
    const p = createSwFetchPathProbe({
      hasController: () => true,
      fetchUrl: (url) => {
        if (!url.includes(FETCH_PROBE_PARAM)) return Promise.resolve(new Response('ok'));
        forwardedCalls += 1;
        return forwardedCalls === 1 ? NEVER : Promise.resolve(new Response('ok'));
      },
      healthUrl: HEALTH,
      recover,
      onSuspected,
      onWedged,
    });
    await runToDeadline(p);
    expect(forwardedCalls).toBe(2);
    expect(onSuspected).toHaveBeenCalledTimes(1);
    expect(recover).not.toHaveBeenCalled();
  });

  it('gives no evidence when the page went hidden during the window', async () => {
    const p = probe({ direct: 'answers', forwarded: 'hangs' });
    const done = p.check();
    setVisibility('hidden');
    setVisibility('visible');
    await vi.advanceTimersByTimeAsync(2 * SW_FETCH_PROBE_TIMEOUT_MS);
    await done;
    expect(recover).not.toHaveBeenCalled();
  });

  it('recovers at most once per guard window', async () => {
    await runToDeadline(probe({ direct: 'answers', forwarded: 'hangs' }));
    await runToDeadline(probe({ direct: 'answers', forwarded: 'hangs' }));
    expect(recover).toHaveBeenCalledTimes(1);
  });

  it('does not recover after stop', async () => {
    const p = probe({ direct: 'answers', forwarded: 'hangs' });
    const done = p.check();
    p.stop();
    await vi.advanceTimersByTimeAsync(2 * SW_FETCH_PROBE_TIMEOUT_MS);
    await done;
    expect(recover).not.toHaveBeenCalled();
  });
});

describe('claimFetchPathRecovery', () => {
  it('allows one claim per window and another after it', () => {
    expect(claimFetchPathRecovery(1_000_000)).toBe(true);
    expect(claimFetchPathRecovery(1_000_000 + SW_FETCH_RECOVERY_WINDOW_MS - 1)).toBe(false);
    expect(claimFetchPathRecovery(1_000_000 + SW_FETCH_RECOVERY_WINDOW_MS)).toBe(true);
  });

  it('refuses when sessionStorage is unavailable, so a reload cannot loop', () => {
    const spy = vi.spyOn(globalThis.sessionStorage, 'getItem').mockImplementation(() => { throw new Error('denied'); });
    expect(claimFetchPathRecovery(1_000_000)).toBe(false);
    spy.mockRestore();
  });
});

describe('fetchProbeUrls', () => {
  it('marks only the forwarded URL', () => {
    expect(fetchProbeUrls(HEALTH)).toEqual({ direct: HEALTH, forwarded: `${HEALTH}?sw-fetch-probe=1` });
  });
});
