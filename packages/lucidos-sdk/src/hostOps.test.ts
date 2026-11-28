import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { installHostOps } from './hostOps';
import { _resetHostServingForTesting } from './_bridge';

const APP_DOC = 'https://host.example/ws/app/pr-understanding/';
const HOST_ORIGIN = new URL(APP_DOC).origin;

const saved: Record<string, unknown> = {};
let listener: ((event: MessageEvent) => void) | null = null;
let loc: { href: string; origin: string; replace: ReturnType<typeof vi.fn>; hash?: string };

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
  Object.defineProperty(globalThis, 'location', { value: loc, configurable: true, writable: true });
  installHostOps();
  return parent;
}

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
