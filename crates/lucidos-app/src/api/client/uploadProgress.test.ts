import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

import {
  postWithUploadProgress,
  sendWithUploadProgress,
  uploadErrorMessage,
  UPLOAD_RESPONSE_MS,
  UPLOAD_STALLED_MESSAGE,
  UPLOAD_STALL_MS,
  UPLOAD_TOO_LARGE_MESSAGE,
  UPLOAD_UNREACHABLE_MESSAGE,
} from './uploadProgress';
import { SdkError } from '../../../../../packages/lucidos-sdk/src/_fetch';
import { ApiError, isTransientFetchError, throwIfNotOk } from './_core';

import { observeSourceGeneration } from '../../utils/restoreSourceGeneration';
import { installRestoreAdmission } from '../../utils/restoreAdmission';
import { FakeXhr, installFakeXhr, sent, uninstallFakeXhr } from './__tests__/fakeXhr';

describe('postWithUploadProgress', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    FakeXhr.last = null;
    (globalThis as any).XMLHttpRequest = FakeXhr;
  });
  afterEach(() => {
    observeSourceGeneration(null);
    installRestoreAdmission({ ready: () => true, phase: () => 'admitted', wait: async () => {} });
    vi.useRealTimers();
    delete (globalThis as any).XMLHttpRequest;
  });

  it('rejects uploads waiting across a restore without sending bytes', async () => {
    observeSourceGeneration(null);
    let release!: () => void;
    let ready = false;
    const wait = new Promise<void>((resolve) => { release = resolve; });
    installRestoreAdmission({ ready: () => ready, phase: () => ready ? 'admitted' : 'awaiting-hello', wait: () => wait });
    const request = postWithUploadProgress('/api/v1/threads/t/blobs', new FormData());
    const rejected = expect(request).rejects.toMatchObject({ httpCode: 409 });
    observeSourceGeneration('00000000-0000-4000-8000-000000000001');
    ready = true;
    release();
    await rejected;
    expect(FakeXhr.last).toBeNull();
  });

  it('rejects an upload response from an earlier generation', async () => {
    const request = postWithUploadProgress('/api/v1/threads/t/blobs', new FormData());
    const rejected = expect(request).rejects.toMatchObject({ httpCode: 409 });
    const xhr = await sent();
    expect(xhr.requestHeaders['x-lucidos-restore-generation']).toBe('none');
    observeSourceGeneration('00000000-0000-4000-8000-000000000001');
    xhr.respond(201, '{}');
    await rejected;
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

describe('sendWithUploadProgress', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    installFakeXhr();
  });
  afterEach(() => {
    vi.useRealTimers();
    uninstallFakeXhr();
  });

  it('sends the method and headers it is given, and no workspace headers', async () => {
    const p = sendWithUploadProgress({ method: 'PATCH', url: '/~/u', headers: { 'Upload-Offset': '0' }, body: new Blob(['x']) });
    const xhr = await sent();
    expect(xhr.method).toBe('PATCH');
    expect(xhr.requestHeaders).toEqual({ 'Upload-Offset': '0' });
    xhr.respond(204, '');
    expect((await p).status).toBe(204);
  });

  it('rejects at once when the caller already cancelled', async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(sendWithUploadProgress({ method: 'POST', url: '/u', body: 'x' }, { signal: controller.signal })).rejects.toBeTruthy();
    expect(FakeXhr.last).toBeNull();
  });
});

describe('uploadErrorMessage', () => {
  async function refusal(status: number, body: string, contentType = 'text/html'): Promise<unknown> {
    return throwIfNotOk(new Response(body, { status, headers: { 'content-type': contentType } })).catch((e) => e);
  }

  it('names a proxy body cap for an HTML 413', async () => {
    expect(uploadErrorMessage(await refusal(413, '<html>Request Entity Too Large</html>'))).toBe(UPLOAD_TOO_LARGE_MESSAGE);
  });

  it('names a proxy that gave up for an HTML 502 or 504', async () => {
    expect(uploadErrorMessage(await refusal(502, '<html>Bad gateway</html>'))).toContain('HTTP 502');
    expect(uploadErrorMessage(await refusal(504, '<html>Gateway timeout</html>'))).toContain('HTTP 504');
  });

  it("keeps Lucidos's own sentence when the refusal carries one", async () => {
    const err = await refusal(502, '{"error":"lab refused the request"}', 'application/json');
    expect(uploadErrorMessage(err)).toBe('lab refused the request');
  });

  it('maps an SDK refusal by its status too', () => {
    expect(uploadErrorMessage(new SdkError(413, 'Payload Too Large'))).toBe(UPLOAD_TOO_LARGE_MESSAGE);
  });

  it('says Lucidos could not be reached for a dropped connection', () => {
    expect(uploadErrorMessage(new TypeError('Failed to fetch'))).toBe(UPLOAD_UNREACHABLE_MESSAGE);
  });

  it('passes a stall through with its own sentence', () => {
    expect(uploadErrorMessage(new DOMException(UPLOAD_STALLED_MESSAGE, 'TimeoutError'))).toBe(UPLOAD_STALLED_MESSAGE);
  });
});
