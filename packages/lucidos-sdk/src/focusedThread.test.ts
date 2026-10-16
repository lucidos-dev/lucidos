import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { BRIDGE_PUSH_TYPE, BRIDGE_REPLY_TYPE, BRIDGE_TYPE, FOCUSED_THREAD_CHANNEL } from './_bridge';

const HOST_ORIGIN = 'https://localhost:5251';

const saved: Record<string, unknown> = {};
let listeners: Array<(event: MessageEvent) => void> = [];
let parent: { postMessage: ReturnType<typeof vi.fn> };

/** A fresh SDK module graph per case: the bridge installs its listener once
 *  per module instance, on the window present at that moment. */
async function load() {
  vi.resetModules();
  return import('./focusedThread');
}

function fromWindow(source: unknown, data: unknown, origin = HOST_ORIGIN): void {
  for (const fn of listeners) fn({ source, origin, data } as unknown as MessageEvent);
}

const push = (data: unknown, source: unknown = parent, origin = HOST_ORIGIN) =>
  fromWindow(source, { type: BRIDGE_PUSH_TYPE, channel: FOCUSED_THREAD_CHANNEL, data }, origin);

beforeEach(() => {
  saved.window = (globalThis as any).window;
  saved.location = Object.getOwnPropertyDescriptor(globalThis, 'location');
  listeners = [];
  parent = { postMessage: vi.fn() };
  (globalThis as any).window = {
    parent,
    addEventListener: (type: string, fn: (e: MessageEvent) => void) => {
      if (type === 'message') listeners.push(fn);
    },
  };
  Object.defineProperty(globalThis, 'location', {
    value: { origin: HOST_ORIGIN, href: `${HOST_ORIGIN}/app/habit-tracker/` },
    configurable: true,
    writable: true,
  });
});

afterEach(() => {
  (globalThis as any).window = saved.window;
  if (saved.location) Object.defineProperty(globalThis, 'location', saved.location as PropertyDescriptor);
  else delete (globalThis as any).location;
});

describe('focusedThread', () => {
  /** The request the SDK posted, answered as the host would. */
  function hostAnswers(value: unknown): void {
    const [request, target] = parent.postMessage.mock.calls[0];
    expect(request).toMatchObject({ type: BRIDGE_TYPE, op: 'ui.focused-thread' });
    expect(target).toBe(HOST_ORIGIN);
    fromWindow(parent, { type: BRIDGE_REPLY_TYPE, id: request.id, ok: true, value });
  }

  it('asks the host and answers its thread id', async () => {
    const { focusedThread } = await load();
    const answer = focusedThread();
    hostAnswers('thread-a');
    await expect(answer).resolves.toBe('thread-a');
  });

  it('answers null when the host has no thread open', async () => {
    const { focusedThread } = await load();
    const answer = focusedThread();
    hostAnswers(null);
    await expect(answer).resolves.toBeNull();
  });

  it('rejects an answer that is not a thread id', async () => {
    const { focusedThread } = await load();
    const answer = focusedThread();
    hostAnswers(42);
    await expect(answer).rejects.toThrow(/no thread id/);
  });

  it('rejects with no host, rather than claiming no thread is open', async () => {
    (globalThis as any).window.parent = (globalThis as any).window;
    const { focusedThread } = await load();
    await expect(focusedThread()).rejects.toThrow(/no host/);
  });
});

describe('onFocusedThreadChange', () => {
  it('calls back with each id the host pushes, null included', async () => {
    const { onFocusedThreadChange } = await load();
    const seen: Array<string | null> = [];
    onFocusedThreadChange((id) => seen.push(id));
    push({ threadId: 'thread-a' });
    push({ threadId: 'thread-b' });
    push({ threadId: null });
    expect(seen).toEqual(['thread-a', 'thread-b', null]);
  });

  it('stops after the unsubscribe', async () => {
    const { onFocusedThreadChange } = await load();
    const seen: Array<string | null> = [];
    const stop = onFocusedThreadChange((id) => seen.push(id));
    push({ threadId: 'thread-a' });
    stop();
    push({ threadId: 'thread-b' });
    expect(seen).toEqual(['thread-a']);
  });

  it('ignores a push from a nested frame or another origin', async () => {
    const { onFocusedThreadChange } = await load();
    const seen: Array<string | null> = [];
    onFocusedThreadChange((id) => seen.push(id));
    push({ threadId: 'forged' }, { postMessage() {} });
    push({ threadId: 'foreign' }, parent, 'https://foreign.example');
    expect(seen).toEqual([]);
  });

  it('drops a push with no thread id', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const { onFocusedThreadChange } = await load();
    const seen: Array<string | null> = [];
    onFocusedThreadChange((id) => seen.push(id));
    push({ threadId: 7 });
    push(null);
    expect(seen).toEqual([]);
    expect(warn).toHaveBeenCalledTimes(2);
    warn.mockRestore();
  });

  it('refuses a callback that is not a function', async () => {
    const { onFocusedThreadChange } = await load();
    expect(() => onFocusedThreadChange('nope' as never)).toThrow(TypeError);
  });
});
