import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import { postWithUploadProgress, UPLOAD_STALL_MS, UPLOAD_RESPONSE_MS } from './uploadProgress';
import { ApiError, isTransientFetchError, throwIfNotOk } from './_core';

type Handler = ((e: any) => void) | null;

/** Just enough of `XMLHttpRequest` to drive every path the transport takes. */
class FakeXhr {
  static last: FakeXhr | null = null;
  upload: { onprogress: Handler; onload: Handler } = { onprogress: null, onload: null };
  onload: Handler = null;
  onerror: Handler = null;
  onabort: Handler = null;
  method = '';
  url = '';
  requestHeaders: Record<string, string> = {};
  body: unknown = null;
  aborted = false;
  status = 0;
  statusText = '';
  responseText = '';
  private rawHeaders = '';

  open(method: string, url: string) { this.method = method; this.url = url; }
  setRequestHeader(k: string, v: string) { this.requestHeaders[k] = v; }
  send(body: unknown) { this.body = body; FakeXhr.last = this; }
  abort() { this.aborted = true; this.onabort?.({}); }
  getAllResponseHeaders() { return this.rawHeaders; }

  progress(loaded: number, total: number) {
    this.upload.onprogress?.({ lengthComputable: true, loaded, total });
  }
  bodySent() { this.upload.onload?.({}); }
  respond(status: number, body: string, headers: Record<string, string> = {}) {
    this.status = status;
    this.responseText = body;
    this.rawHeaders = Object.entries(headers).map(([k, v]) => `${k}: ${v}`).join('\r\n');
    this.onload?.({});
  }
  dropConnection() { this.onerror?.({}); }
}

function current(): FakeXhr {
  if (!FakeXhr.last) throw new Error('no request was sent');
  return FakeXhr.last;
}

/** The transport awaits device registration before opening the request. */
async function sent(): Promise<FakeXhr> {
  await vi.advanceTimersByTimeAsync(0);
  return current();
}

describe('postWithUploadProgress', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    FakeXhr.last = null;
    (globalThis as any).XMLHttpRequest = FakeXhr;
  });
  afterEach(() => {
    vi.useRealTimers();
    delete (globalThis as any).XMLHttpRequest;
  });

  it('reports progress and the moment the body has left', async () => {
    const onProgress = vi.fn();
    const onBodySent = vi.fn();
    const p = postWithUploadProgress('/api/v1/threads/t/blobs', new FormData(), { onProgress, onBodySent });
    const xhr = await sent();
    expect(xhr.method).toBe('POST');
    xhr.progress(10, 100);
    xhr.progress(100, 100);
    xhr.bodySent();
    xhr.respond(201, '{"hash":"h"}', { 'content-type': 'application/json' });
    const res = await p;
    expect(onProgress.mock.calls.map((c) => c[0])).toEqual([
      { sentBytes: 10, totalBytes: 100 },
      { sentBytes: 100, totalBytes: 100 },
    ]);
    expect(onBodySent).toHaveBeenCalledTimes(1);
    expect(res.status).toBe(201);
    expect(await res.json()).toEqual({ hash: 'h' });
  });

  it('times out a body that stops making progress, as a transient failure', async () => {
    const p = postWithUploadProgress('/u', new FormData());
    const rejection = expect(p).rejects.toSatisfy((err: unknown) => isTransientFetchError(err)
      && (err as DOMException).name === 'TimeoutError');
    const xhr = await sent();
    // Progress keeps the deadline alive.
    await vi.advanceTimersByTimeAsync(UPLOAD_STALL_MS - 1);
    xhr.progress(5, 100);
    await vi.advanceTimersByTimeAsync(UPLOAD_STALL_MS - 1);
    expect(xhr.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(xhr.aborted).toBe(true);
    await rejection;
  });

  it('times out an engine that never answers a fully sent body', async () => {
    const p = postWithUploadProgress('/u', new FormData());
    const rejection = expect(p).rejects.toSatisfy((err: unknown) => (err as DOMException).name === 'TimeoutError');
    const xhr = await sent();
    xhr.bodySent();
    // The stall deadline no longer applies once the body is out.
    await vi.advanceTimersByTimeAsync(UPLOAD_RESPONSE_MS - 1);
    expect(xhr.aborted).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(xhr.aborted).toBe(true);
    await rejection;
  });

  it('reports a dropped connection as a transport error', async () => {
    const p = postWithUploadProgress('/u', new FormData());
    const rejection = expect(p).rejects.toSatisfy((err: unknown) => isTransientFetchError(err));
    (await sent()).dropConnection();
    await rejection;
  });

  it('aborts the request when the caller cancels', async () => {
    const controller = new AbortController();
    const p = postWithUploadProgress('/u', new FormData(), { signal: controller.signal });
    const rejection = expect(p).rejects.toSatisfy((err: unknown) => (err as DOMException).name === 'AbortError');
    const xhr = await sent();
    controller.abort();
    expect(xhr.aborted).toBe(true);
    await rejection;
  });

  it('never opens a request for a signal that is already aborted', async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(postWithUploadProgress('/u', new FormData(), { signal: controller.signal })).rejects.toBeTruthy();
    expect(FakeXhr.last).toBeNull();
  });

  it('keeps the error normalization: a JSON refusal reads its sentence', async () => {
    const p = postWithUploadProgress('/u', new FormData());
    (await sent()).respond(415, '{"error":"That file is a PDF, not an image"}', { 'content-type': 'application/json' });
    const err = await throwIfNotOk(await p).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).reason).toBe('That file is a PDF, not an image');
    expect(isTransientFetchError(err)).toBe(false);
  });

  it('keeps the error normalization: the gateway boot splash is transient and never markup', async () => {
    const p = postWithUploadProgress('/u', new FormData());
    (await sent()).respond(503, '<!doctype html><meta charset="utf-8"><title>Starting</title>', {
      'content-type': 'text/html',
      'x-lucidos-boot-splash': '1',
    });
    const err = await throwIfNotOk(await p).catch((e) => e);
    expect(isTransientFetchError(err)).toBe(true);
    expect((err as ApiError).reason).not.toContain('<');
  });
});
