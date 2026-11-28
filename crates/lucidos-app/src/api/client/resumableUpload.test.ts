import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  discardResumableUpload,
  FIRST_CHUNK_BYTES,
  FLOOR_CHUNK_BYTES,
  ProxyCapError,
  uploadResumable,
} from './resumableUpload';
import {
  fakeAbortSignalTimeout,
  fakeTusServer,
  fileOf,
  runUntilSettled,
  useUploadFakeTimers,
} from './__tests__/fakeTusServer';
import { installXhrOverFetch, uninstallXhrOverFetch } from './__tests__/xhrOverFetch';

const BASE = '/~/api/v1/control/restore-uploads';
const MiB = 1024 * 1024;
const originalFetch = globalThis.fetch;

function serve(server: ReturnType<typeof fakeTusServer>) {
  globalThis.fetch = server.fetch;
  return server;
}

async function digestOf(bytes: Uint8Array<ArrayBuffer>): Promise<string> {
  return btoa(String.fromCharCode(...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))));
}

function patches(server: ReturnType<typeof fakeTusServer>) {
  return server.requests.filter((r) => r.method === 'PATCH');
}

beforeEach(() => {
  localStorage.clear();
  installXhrOverFetch();
});
afterEach(() => {
  uninstallXhrOverFetch();
  globalThis.fetch = originalFetch;
  vi.useRealTimers();
  vi.restoreAllMocks();
});

async function stopAfterFirstMiB(file: File): Promise<void> {
  const controller = new AbortController();
  await expect(uploadResumable({
    base: BASE,
    file,
    signal: controller.signal,
    onProgress: (p) => { if (p.sentBytes >= MiB) controller.abort(); },
  })).rejects.toBeTruthy();
}

describe('no request carries more than one chunk, and the chunk adapts', () => {
  it('halves on 413 down to a 1 MiB proxy cap and completes the file intact', async () => {
    const server = serve(fakeTusServer({ capBytes: MiB }));
    const file = fileOf(33 * MiB);
    const id = await uploadResumable({ base: BASE, file });
    const sizes = patches(server).map((r) => r.bodyBytes);
    expect(sizes.slice(0, 6)).toEqual([32, 16, 8, 4, 2, 1].map((n) => n * MiB));
    expect(sizes.slice(5).every((n) => n === MiB)).toBe(true);
    expect(await digestOf(server.sessions.get(id)!.bytes)).toBe(await digestOf(new Uint8Array(await file.arrayBuffer())));
  });

  it('sends 32 MiB chunks through a 100 MB cap without a single refusal', async () => {
    const server = serve(fakeTusServer({ capBytes: 100_000_000 }));
    await uploadResumable({ base: BASE, file: fileOf(40 * MiB) });
    expect(patches(server).map((r) => r.bodyBytes)).toEqual([FIRST_CHUNK_BYTES, 8 * MiB]);
  });

  it('names the proxy cap when even the floor chunk is refused', async () => {
    const server = serve(fakeTusServer({ capBytes: FLOOR_CHUNK_BYTES - 1 }));
    await expect(uploadResumable({ base: BASE, file: fileOf(MiB) })).rejects.toBeInstanceOf(ProxyCapError);
    expect(Math.min(...patches(server).map((r) => r.bodyBytes))).toBe(FLOOR_CHUNK_BYTES);
  });

  it('halves after a chunk times out, so a slow uplink still finishes', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ stallPatchesOver: 8 * MiB }));
    const done = uploadResumable({ base: BASE, file: fileOf(20 * MiB) });
    await runUntilSettled(done);
    await done;
    expect(patches(server).map((r) => r.bodyBytes).slice(0, 3)).toEqual([20, 16, 8].map((n) => n * MiB));
  });

  it('a chunk the gateway is still reading is never taken for a stall', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ stallPatchesOver: 8 * MiB, bodyReadTurns: 1000 }));
    const done = uploadResumable({ base: BASE, file: fileOf(20 * MiB) });
    await runUntilSettled(done);
    await done;
    expect(patches(server).map((r) => r.bodyBytes)).toEqual([20, 16, 8, 8, 4].map((n) => n * MiB));
  });

  it('a chunk that only passes at the floor still finishes within the retry budget', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ stallPatchesOver: FLOOR_CHUNK_BYTES }));
    const done = uploadResumable({ base: BASE, file: fileOf(2 * MiB) });
    await runUntilSettled(done);
    await done;
    expect(Math.min(...patches(server).map((r) => r.bodyBytes))).toBe(FLOOR_CHUNK_BYTES);
  });

  it('labels each chunk with its SHA-256 when the page can hash', async () => {
    const server = serve(fakeTusServer());
    const file = fileOf(1000);
    await uploadResumable({ base: BASE, file });
    const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', await file.arrayBuffer()));
    const expected = `sha256 ${btoa(String.fromCharCode(...digest))}`;
    expect(patches(server)[0].headers['upload-checksum']).toBe(expected);
  });
});

describe('an upload resumes from the gateway offset', () => {
  it('picking the same file again starts with HEAD and sends only the rest', async () => {
    const server = serve(fakeTusServer({ capBytes: MiB }));
    const file = fileOf(3 * MiB);
    const controller = new AbortController();
    const first = uploadResumable({
      base: BASE,
      file,
      signal: controller.signal,
      onProgress: (p) => { if (p.sentBytes >= MiB) controller.abort(); },
    });
    await expect(first).rejects.toBeTruthy();
    const [session] = server.sessions.values();
    expect(session.offset).toBe(MiB);

    server.requests.length = 0;
    await uploadResumable({ base: BASE, file });
    expect(server.requests[0].method).toBe('HEAD');
    expect(server.requests.some((r) => r.method === 'POST')).toBe(false);
    expect(patches(server).find((r) => r.bodyBytes <= MiB)?.headers['upload-offset']).toBe(String(MiB));
    expect(session.offset).toBe(3 * MiB);
  });

  it('a session the gateway no longer has starts a new one', async () => {
    const server = serve(fakeTusServer());
    const file = fileOf(1000);
    await uploadResumable({ base: BASE, file });
    server.sessions.clear();
    server.requests.length = 0;
    await uploadResumable({ base: BASE, file });
    expect(server.requests.map((r) => r.method).slice(0, 2)).toEqual(['HEAD', 'POST']);
  });

  it('a gateway that fails the resume HEAD keeps the session and resumes it', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ capBytes: MiB }));
    const file = fileOf(3 * MiB);
    await stopAfterFirstMiB(file);
    const [[id, session]] = server.sessions.entries();
    const flaky = serve(fakeTusServer({ capBytes: MiB, failHeads: 1 }));
    flaky.sessions.set(id, session);
    const done = uploadResumable({ base: BASE, file });
    await runUntilSettled(done);
    await done;
    expect(flaky.requests.some((r) => r.method === 'POST')).toBe(false);
    expect(session.offset).toBe(3 * MiB);
  });

  it('a session creation the gateway never answers fails with a timeout', async () => {
    useUploadFakeTimers();
    fakeAbortSignalTimeout();
    serve(fakeTusServer({ hangCreate: true }));
    const done = uploadResumable({ base: BASE, file: fileOf(1000) });
    await runUntilSettled(done);
    await expect(done).rejects.toMatchObject({ name: 'TimeoutError' });
  });

  it('discarding ends the gateway session and forgets it', async () => {
    const server = serve(fakeTusServer({ capBytes: MiB }));
    const file = fileOf(3 * MiB);
    await stopAfterFirstMiB(file);
    await discardResumableUpload(BASE, file);
    expect(server.requests[server.requests.length - 1]?.method).toBe('DELETE');
    expect(server.sessions.size).toBe(0);
    expect(localStorage.length).toBe(0);
  });

  it('discarding waits out a write that still holds the session', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ capBytes: MiB }));
    const file = fileOf(3 * MiB);
    await stopAfterFirstMiB(file);
    const busy = serve(fakeTusServer({ busyDeletes: 1 }));
    busy.sessions.set(...[...server.sessions.entries()][0]);
    const done = discardResumableUpload(BASE, file);
    await runUntilSettled(done);
    await done;
    expect(busy.requests.filter((r) => r.method === 'DELETE')).toHaveLength(2);
    expect(busy.sessions.size).toBe(0);
    expect(localStorage.length).toBe(0);
  });

  it('a discard the gateway keeps refusing remembers the session for another try', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ capBytes: MiB }));
    const file = fileOf(3 * MiB);
    await stopAfterFirstMiB(file);
    const busy = serve(fakeTusServer({ busyDeletes: Infinity }));
    busy.sessions.set(...[...server.sessions.entries()][0]);
    const done = discardResumableUpload(BASE, file);
    await runUntilSettled(done);
    await expect(done).rejects.toBeTruthy();
    expect(busy.sessions.size).toBe(1);
    expect(localStorage.length).toBe(1);
  });

  it('a dropped connection retries after a pause, re-reading the offset', async () => {
    useUploadFakeTimers();
    const server = serve(fakeTusServer({ dropPatches: 1 }));
    const retries: number[] = [];
    const done = uploadResumable({ base: BASE, file: fileOf(1000), onRetry: (n) => retries.push(n) });
    await runUntilSettled(done);
    await done;
    expect(retries).toEqual([1]);
    expect(server.requests.map((r) => r.method)).toEqual(['POST', 'PATCH', 'HEAD', 'PATCH']);
  });

  it('gives up with the last error once the retries run out', async () => {
    useUploadFakeTimers();
    serve(fakeTusServer({ dropPatches: 99 }));
    const done = uploadResumable({ base: BASE, file: fileOf(1000) });
    await runUntilSettled(done);
    await expect(done).rejects.toThrow('Failed to fetch');
  });
});
