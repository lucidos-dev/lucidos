// @vitest-environment jsdom
/**
 * The host's half of keeping an app frame's URL pass alive (ADR 0238).
 *
 * jsdom, because a round walks the mounted `iframe[data-role="app-ui-frame"]`
 * elements and posts to each one's `contentWindow`. Neither is expressible
 * against the suite's default document stub.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  artifactPreviewCapability,
  renewEveryOpenFrame,
  resetArtifactPreviewCapability,
  stopFrameCapabilityRenewal,
} from './frame-capability';

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
  resetArtifactPreviewCapability();
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

function mountPreviewFrame(): HTMLIFrameElement {
  const frame = document.createElement('iframe');
  frame.setAttribute('data-role', 'artifact-preview-frame');
  document.body.appendChild(frame);
  return frame;
}

/** A pass the way the engine writes one: expiry in hex seconds, then subject
 *  and signature. */
function passExpiringIn(secs: number, tag: string): string {
  return `${Math.floor(Date.now() / 1000 + secs).toString(16)}~artifact..preview~${tag}`;
}

describe('the artifact preview pass', () => {
  it('is minted from its own host-only route, and cached while it is fresh', async () => {
    const asked: string[] = [];
    vi.stubGlobal('fetch', vi.fn(async (url: string) => {
      asked.push(url);
      return { ok: true, json: async () => ({ capability: passExpiringIn(3600, 'a'), renew_after_secs: 1800 }) };
    }));

    const first = await artifactPreviewCapability();
    const second = await artifactPreviewCapability();

    expect(asked).toEqual(['/api/v1/artifact-preview-capability']);
    expect(second).toBe(first);
  });

  it('is re-minted once it is close to lapsing, so a laptop that slept gets a live one', async () => {
    let n = 0;
    vi.stubGlobal('fetch', vi.fn(async () => {
      n += 1;
      return { ok: true, json: async () => ({ capability: passExpiringIn(3600, `p${n}`), renew_after_secs: 1800 }) };
    }));

    const first = await artifactPreviewCapability();
    vi.advanceTimersByTime(56 * 60 * 1000);
    const later = await artifactPreviewCapability();

    expect(later).not.toBe(first);
    expect(later).toContain('~p2');
  });

  it('is null with no gateway in front', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ capability: null, renew_after_secs: 1800 }) })));
    expect(await artifactPreviewCapability()).toBeNull();
  });

  it('is renewed into every mounted preview frame by the same round', async () => {
    const one = capture(mountPreviewFrame());
    const two = capture(mountPreviewFrame());
    const pass = passExpiringIn(3600, 'renewed');
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: true, json: async () => ({ capability: pass, renew_after_secs: 1800 }) })));

    const nextDelayMs = await renewEveryOpenFrame();

    const pushed = { type: 'lucidos:preview-host', kind: 'capability', capability: pass };
    expect(one).toEqual([pushed]);
    expect(two).toEqual([pushed]);
    expect(nextDelayMs).toBe(1_800_000);
    // The round also refreshed the cache the next preview builds from.
    expect(await artifactPreviewCapability()).toBe(pass);
  });

  it('retries sooner when the preview renewal fails', async () => {
    const seen = capture(mountPreviewFrame());
    vi.stubGlobal('fetch', vi.fn(async () => ({ ok: false, status: 503 })));
    vi.spyOn(console, 'warn').mockImplementation(() => {});

    expect(await renewEveryOpenFrame()).toBe(60_000);
    expect(seen).toEqual([]);
  });
});
