/**
 * The app half of the bridge.
 *
 * Two things are worth pinning here. Whether this frame is isolated at all,
 * because every seam branches on it and the two obvious tests are both wrong.
 * And the two wire conversions, because a `Response` and a `FormData` cannot
 * cross a structured clone and every SDK caller expects them back intact.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  isBridged,
  toWireBody,
  fromWireResponse,
  headersToRecord,
  _setBridgedForTesting,
} from './_bridge';

describe('isBridged', () => {
  const saved: Record<string, unknown> = {};

  beforeEach(() => {
    _setBridgedForTesting(null);
    saved.window = (globalThis as any).window;
    saved.origin = Object.getOwnPropertyDescriptor(globalThis, 'origin');
  });

  afterEach(() => {
    _setBridgedForTesting(null);
    (globalThis as any).window = saved.window;
    if (saved.origin) {
      Object.defineProperty(globalThis, 'origin', saved.origin as PropertyDescriptor);
    } else {
      delete (globalThis as any).origin;
    }
  });

  function setFrame(opts: { hasParent: boolean; origin: string }) {
    const win: any = { addEventListener() {} };
    win.parent = opts.hasParent ? { postMessage() {} } : win;
    (globalThis as any).window = win;
    Object.defineProperty(globalThis, 'origin', {
      value: opts.origin,
      configurable: true,
      writable: true,
    });
  }

  it('is true in a frame with a parent and an opaque origin', () => {
    setFrame({ hasParent: true, origin: 'null' });
    expect(isBridged()).toBe(true);
  });

  it('is false in a standalone tab, which has no parent to bridge to', () => {
    setFrame({ hasParent: false, origin: 'https://localhost:5251' });
    expect(isBridged()).toBe(false);
  });

  it('is false in an ordinary same-origin frame, which needs no bridge', () => {
    // The trap: a parent check alone is true here too.
    setFrame({ hasParent: true, origin: 'https://localhost:5251' });
    expect(isBridged()).toBe(false);
  });
});

describe('toWireBody', () => {
  it('carries nothing for an absent body', () => {
    expect(toWireBody(undefined)).toEqual({ kind: 'none' });
    expect(toWireBody(null)).toEqual({ kind: 'none' });
  });

  it('carries a string as text', () => {
    expect(toWireBody('{"a":1}')).toEqual({ kind: 'text', value: '{"a":1}' });
  });

  it('flattens FormData, which structured clone refuses', () => {
    const form = new FormData();
    form.append('name', 'report');
    const wire = toWireBody(form);
    expect(wire.kind).toBe('form');
    expect((wire as { entries: Array<[string, unknown]> }).entries).toEqual([['name', 'report']]);
  });

  it('carries a URLSearchParams by its encoded form', () => {
    expect(toWireBody(new URLSearchParams({ a: '1' }))).toEqual({ kind: 'text', value: 'a=1' });
  });

  it('carries a typed-array VIEW by its own window, not the whole buffer', () => {
    // A subarray is a view onto part of a larger buffer. Copying the buffer
    // would send bytes either side of it, which for an upload is somebody
    // else's data.
    const backing = new Uint8Array([1, 2, 3, 4, 5, 6]);
    const view = backing.subarray(2, 4);
    const wire = toWireBody(view) as { kind: string; value: ArrayBuffer };

    expect(wire.kind).toBe('binary');
    expect([...new Uint8Array(wire.value)]).toEqual([3, 4]);
  });

  it('refuses a shape it would otherwise drop in silence', () => {
    expect(() => toWireBody(new ReadableStream() as unknown as BodyInit)).toThrow(TypeError);
  });
});

describe('headersToRecord', () => {
  // `HeadersInit` is three shapes and only the record survives a spread. The
  // other two used to vanish without a word, which on a proxy call meant an
  // Authorization header silently not sent.
  it('carries a plain record', () => {
    expect(headersToRecord({ 'X-A': '1' })).toEqual({ 'x-a': '1' });
  });

  it('carries a Headers instance', () => {
    expect(headersToRecord(new Headers({ 'X-A': '1' }))).toEqual({ 'x-a': '1' });
  });

  it('carries an array of pairs', () => {
    expect(headersToRecord([['X-A', '1']])).toEqual({ 'x-a': '1' });
  });

  it('carries nothing for an absent init', () => {
    expect(headersToRecord(undefined)).toEqual({});
  });
});

describe('fromWireResponse', () => {
  const wire = (status: number) => ({
    status,
    statusText: '',
    headers: { 'content-type': 'application/json' },
    body: new TextEncoder().encode('{"ok":true}').buffer as ArrayBuffer,
  });

  it('rebuilds a response a caller can read', async () => {
    const res = fromWireResponse(wire(200));
    expect(res.ok).toBe(true);
    expect(await res.json()).toEqual({ ok: true });
    expect(res.headers.get('content-type')).toBe('application/json');
  });

  it('keeps a failure a failure, so the SDK still throws its SdkError', () => {
    expect(fromWireResponse(wire(404)).ok).toBe(false);
  });

  it('drops the body on a status the Response constructor refuses one for', () => {
    // A DELETE answering 204 is the one the SDK meets. Passing a body here
    // throws a TypeError, which would surface as a broken call rather than as
    // the successful delete it was.
    expect(() => fromWireResponse(wire(204))).not.toThrow();
    expect(fromWireResponse(wire(204)).status).toBe(204);
  });
});
