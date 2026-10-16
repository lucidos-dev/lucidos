/**
 * The navigation the frame now does for itself.
 *
 * These cases came from `crates/lucidos-app/src/components/apps/iframeNav.test.ts`
 * unchanged in substance. The host used to drive `contentWindow.location`, an
 * isolated frame denies that, and the arithmetic moved into the frame's realm
 * with the code. The bug they guard did not move: a pushed history entry is
 * replayed by the iOS edge-swipe-back gesture (WebKit #9166).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { installHostOps } from './hostOps';
import { _resetHostServingForTesting } from './_bridge';

const APP_DOC = 'https://host.example/ws/app/pr-understanding/';
const HOST_ORIGIN = new URL(APP_DOC).origin;

const saved: Record<string, unknown> = {};
let listener: ((event: MessageEvent) => void) | null = null;
let loc: { href: string; origin: string; replace: ReturnType<typeof vi.fn>; hash?: string };

/** Install a frame whose live URL can drift, the way a real app moves itself
 *  with `history.replaceState`, and capture the SDK's message listener. */
function setFrame(hash = '', query = '') {
  loc = {
    href: `${APP_DOC}${query}${hash}`,
    origin: HOST_ORIGIN,
    replace: vi.fn((url: string) => { loc.href = url; }),
  };
  const parent = { postMessage: vi.fn() };
  (globalThis as any).window = {
    parent,
    addEventListener: (type: string, fn: (e: MessageEvent) => void) => {
      if (type === 'message') listener = fn;
    },
  };
  // `defineProperty`, because a plain assignment to `globalThis.location` is
  // silently dropped where the property is not writable.
  Object.defineProperty(globalThis, 'location', { value: loc, configurable: true, writable: true });
  installHostOps();
  return parent;
}

/** Deliver a host request, as `askFrame` / `tellFrame` would.
 *
 *  Awaited, because the SDK dispatches a handler on a microtask so a throwing
 *  one becomes a rejected reply rather than an exception in the listener. */
async function hostSays(op: string, args: unknown, id?: string) {
  const event = {
    data: { type: 'lucidos:bridge:host', id, op, args },
    source: (globalThis as any).window.parent,
    origin: HOST_ORIGIN,
  };
  listener?.(event as unknown as MessageEvent);
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  saved.window = (globalThis as any).window;
  saved.location = Object.getOwnPropertyDescriptor(globalThis, 'location');
  listener = null;
  // The SDK installs its listener once per realm. That is right at runtime, and
  // it would leave every case after the first with no listener at all.
  _resetHostServingForTesting();
});

afterEach(() => {
  (globalThis as any).window = saved.window;
  if (saved.location) {
    Object.defineProperty(globalThis, 'location', saved.location as PropertyDescriptor);
  } else {
    delete (globalThis as any).location;
  }
});

describe('navigate', () => {
  it('is not an op, so even the host cannot send the frame to a URL', async () => {
    // Whatever window frames the app is its parent. An op taking a whole URL
    // would let a foreign page run a `javascript:` URL in the app's origin.
    const parent = setFrame();
    await hostSays('navigate', { url: '/app/notes-app/' }, 'host-1');

    expect(loc.replace).not.toHaveBeenCalled();
    expect(parent.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'host-1', ok: false }),
      expect.anything(),
    );
  });
});

describe('hash', () => {
  it('moves to the fragment without changing the document', async () => {
    // Same document means a fragment navigation: the app is not reloaded and
    // `hashchange` fires. It also means no `load`, which is why the host raises
    // no cover for it.
    setFrame();
    await hostSays('hash', { fragment: 'pr-1645' });

    expect(loc.replace).toHaveBeenCalledWith(`${APP_DOC}#pr-1645`);
    expect(loc.href.split('#')[0]).toBe(APP_DOC);
  });

  it('replaces rather than pushes, so the joint history stays clean', async () => {
    setFrame();
    let pushed = false;
    Object.defineProperty(loc, 'hash', { get: () => '', set: () => { pushed = true; } });

    await hostSays('hash', { fragment: 'pr-1645' });

    expect(pushed).toBe(false);
    expect(loc.replace).toHaveBeenCalledTimes(1);
  });

  it('moves a frame whose live hash has DRIFTED back onto the target', async () => {
    // The same link clicked twice. PR Understanding reflects its selection back
    // with `history.replaceState`. By the second click the frame sits on a
    // different report than the one the link names.
    setFrame('#pr-1700');
    await hostSays('hash', { fragment: 'pr-1645' });

    expect(loc.href).toBe(`${APP_DOC}#pr-1645`);
  });

  it('keeps the frame query, so a WIP preview is not dropped', async () => {
    setFrame('#pr-1700', '?thread_id=wip-7');
    await hostSays('hash', { fragment: 'pr-1645' });

    expect(loc.href).toBe(`${APP_DOC}?thread_id=wip-7#pr-1645`);
  });

  it('carries a `?` inside the fragment, which is not a query', async () => {
    setFrame();
    await hostSays('hash', { fragment: 'report?tab=files' });

    expect(loc.href).toBe(`${APP_DOC}#report?tab=files`);
  });

  it('does nothing when the frame is already on that fragment', async () => {
    // Idempotence is what lets both delivery sites write without fighting.
    setFrame('#pr-1645');
    await hostSays('hash', { fragment: 'pr-1645' });

    expect(loc.replace).not.toHaveBeenCalled();
  });
});

describe('answering', () => {
  it('replies when the host asked for an answer', async () => {
    const parent = setFrame();
    await hostSays('hash', { fragment: 'pr-1645' }, 'host-1');
    await Promise.resolve();
    await Promise.resolve();

    expect(parent.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ type: 'lucidos:bridge:host:reply', id: 'host-1', ok: true }),
      expect.anything(),
    );
  });

  it('stays silent when it did not, which is what a navigation sends', async () => {
    const parent = setFrame();
    await hostSays('hash', { fragment: 'pr-1645' });

    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it('refuses a message from anywhere but the host', async () => {
    setFrame();
    const event = {
      data: { type: 'lucidos:bridge:host', op: 'hash', args: { fragment: 'elsewhere' } },
      source: { postMessage: vi.fn() },
      origin: HOST_ORIGIN,
    };
    listener?.(event as unknown as MessageEvent);

    expect(loc.replace).not.toHaveBeenCalled();
  });
});
