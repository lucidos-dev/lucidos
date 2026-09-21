// @vitest-environment jsdom
/**
 * The host's half of keeping an app frame's URL pass alive (ADR 0238).
 *
 * jsdom, because a round walks the mounted `iframe[data-role="app-ui-frame"]`
 * elements and posts to each one's `contentWindow`. Neither is expressible
 * against the suite's default document stub.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { renewEveryOpenFrame, stopFrameCapabilityRenewal } from './app-frame-capability';

function mountAppFrame(src: string): HTMLIFrameElement {
  const frame = document.createElement('iframe');
  frame.setAttribute('data-role', 'app-ui-frame');
  frame.setAttribute('src', src);
  document.body.appendChild(frame);
  return frame;
}

/** What the engine answered, and what the frame was handed. */
function capture(frame: HTMLIFrameElement): unknown[] {
  const seen: unknown[] = [];
  const target = frame.contentWindow as Window;
  vi.spyOn(target, 'postMessage').mockImplementation(((message: unknown) => {
    seen.push(message);
  }) as typeof target.postMessage);
  return seen;
}

beforeEach(() => {
  document.body.innerHTML = '';
  vi.useFakeTimers();
});

afterEach(() => {
  stopFrameCapabilityRenewal();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('renewEveryOpenFrame', () => {
  it('hands each mounted frame a pass minted for the app the HOST resolved', async () => {
    const publisher = mountAppFrame('/app/site-publisher/');
    const tracker = mountAppFrame('/app/habit-tracker/');
    const toPublisher = capture(publisher);
    const toTracker = capture(tracker);

    const asked: string[] = [];
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      asked.push(url);
      return {
        ok: true,
        json: async () => ({ capability: `fresh-for-${url.split('=')[1]}`, renew_after_secs: 1800 }),
      };
    }));

    await renewEveryOpenFrame();

    // The app id comes from the frame's own `src`, never from the app.
    expect(asked).toEqual([
      '/api/v1/app-frame-capability?app_id=site-publisher',
      '/api/v1/app-frame-capability?app_id=habit-tracker',
    ]);
    expect(toPublisher).toEqual([{
      type: 'lucidos:bridge:push',
      channel: 'frame-capability',
      data: { capability: 'fresh-for-site-publisher' },
    }]);
    expect(toTracker).toEqual([{
      type: 'lucidos:bridge:push',
      channel: 'frame-capability',
      data: { capability: 'fresh-for-habit-tracker' },
    }]);
  });

  it('pushes nothing when the engine says there is no gateway to prove to', async () => {
    const frame = mountAppFrame('/app/site-publisher/');
    const seen = capture(frame);
    vi.stubGlobal('fetch', vi.fn(async () => ({
      ok: true,
      json: async () => ({ capability: null, renew_after_secs: 1800 }),
    })));

    await renewEveryOpenFrame();

    expect(seen).toEqual([]);
  });

  it('retries sooner after a failed round, and does not tear the frame down', async () => {
    const frame = mountAppFrame('/app/site-publisher/');
    const seen = capture(frame);
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: false, status: 503 })));
    vi.spyOn(console, 'warn').mockImplementation(() => {});

    const nextDelayMs = await renewEveryOpenFrame();

    expect(seen).toEqual([]);
    expect(frame.isConnected).toBe(true);
    // A minute, not the half-life. A lapsed pass is a visibly broken app.
    expect(nextDelayMs).toBe(60_000);
    expect(console.warn).toHaveBeenCalledOnce();
  });

  it('does nothing at all with no app open', async () => {
    const fetchSpy = vi.fn();
    vi.stubGlobal('fetch', fetchSpy);
    await renewEveryOpenFrame();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('arms no timer of its own, so a stop cannot be undone by a round in flight', async () => {
    // The loop owns the schedule; a round only says when to come back. Before
    // this split, a round that resolved after `stop` re-armed the timer behind
    // the teardown's back.
    // `capture` stubs the frame's `postMessage`, which jsdom otherwise delivers
    // on a `setTimeout(0)` of its own. That timer would be counted below and
    // read as one this module armed.
    capture(mountAppFrame('/app/site-publisher/'));
    vi.stubGlobal('fetch', vi.fn(async () => ({
      ok: true,
      json: async () => ({ capability: 'fresh', renew_after_secs: 1800 }),
    })));

    await renewEveryOpenFrame();

    expect(vi.getTimerCount()).toBe(0);
  });
});
