import { vi } from 'vitest';

export interface FakeSession {
  length: number;
  offset: number;
  filename: string;
  bytes: Uint8Array<ArrayBuffer>;
}

export interface SeenRequest {
  method: string;
  url: string;
  bodyBytes: number;
  headers: Record<string, string>;
}

export interface FakeTusOptions {
  capBytes?: number;
  dropPatches?: number;
  stallPatches?: boolean;
  stallPatchesOver?: number;
  failHeads?: number;
  hangCreate?: boolean;
  busyDeletes?: number;
  bodyReadTurns?: number;
}

export function fakeTusServer(options: FakeTusOptions = {}) {
  const sessions = new Map<string, FakeSession>();
  const requests: SeenRequest[] = [];
  let next = 0;
  let drops = options.dropPatches ?? 0;
  let headFailures = options.failHeads ?? 0;
  let busyDeletes = options.busyDeletes ?? 0;

  const handle = async (input: RequestInfo | URL, init: RequestInit = {}): Promise<Response> => {
    const url = String(input);
    const method = (init.method ?? 'GET').toUpperCase();
    const headers = Object.fromEntries(
      Object.entries((init.headers ?? {}) as Record<string, string>).map(([k, v]) => [k.toLowerCase(), v]),
    );
    const body = init.body instanceof Blob
      ? new Uint8Array(await holdClockWhile(readBody(init.body, options.bodyReadTurns ?? 0)))
      : null;
    requests.push({ method, url, bodyBytes: body?.length ?? 0, headers });

    if (method === 'POST' && url.endsWith('/restore-uploads')) {
      if (options.hangCreate) return hangUntilAborted(init.signal);
      const id = String(next++).padStart(32, '0');
      const filename = atob((headers['upload-metadata'] ?? '').replace(/^filename /, ''));
      const length = Number(headers['upload-length']);
      sessions.set(id, { length, offset: 0, filename, bytes: new Uint8Array(length) });
      return new Response(null, { status: 201, headers: { Location: `${url}/${id}` } });
    }
    const id = url.split('/').pop() ?? '';
    const session = sessions.get(id);
    if (method === 'HEAD') {
      if (headFailures > 0) {
        headFailures -= 1;
        return new Response(null, { status: 503 });
      }
      if (!session) return new Response(null, { status: 404 });
      return new Response(null, {
        status: 200,
        headers: { 'Upload-Offset': String(session.offset), 'Upload-Length': String(session.length) },
      });
    }
    if (method === 'PATCH') {
      if (options.stallPatches) return new Promise<Response>(() => {});
      if (options.stallPatchesOver !== undefined && body!.length > options.stallPatchesOver) {
        return hangUntilAborted(init.signal);
      }
      if (drops > 0) {
        drops -= 1;
        throw new TypeError('Failed to fetch');
      }
      if (options.capBytes !== undefined && body!.length > options.capBytes) {
        return new Response('<html>413 Request Entity Too Large</html>', {
          status: 413,
          headers: { 'content-type': 'text/html' },
        });
      }
      if (!session) return Response.json({ error: 'No such restore upload' }, { status: 404 });
      if (Number(headers['upload-offset']) !== session.offset) {
        return Response.json({ error: 'wrong offset' }, { status: 409 });
      }
      session.bytes.set(body!, session.offset);
      session.offset += body!.length;
      return new Response(null, { status: 204, headers: { 'Upload-Offset': String(session.offset) } });
    }
    if (method === 'DELETE') {
      if (busyDeletes > 0) {
        busyDeletes -= 1;
        return Response.json({ error: 'Another request is writing to this upload.' }, { status: 409 });
      }
      if (!sessions.delete(id)) return new Response(null, { status: 404 });
      return new Response(null, { status: 204 });
    }
    if (method === 'POST' && url.endsWith('/workspaces/restore')) {
      const commit = JSON.parse(String(init.body)) as { upload: string; name?: string };
      const committed = sessions.get(commit.upload);
      if (!committed || committed.offset !== committed.length) {
        return Response.json({ error: 'The upload is incomplete' }, { status: 409 });
      }
      sessions.delete(commit.upload);
      return Response.json({ id: commit.name ?? 'restored', name: commit.name ?? 'restored' });
    }
    return new Response(null, { status: 404 });
  };

  const fetchMock = vi.fn(handle);
  return { fetch: fetchMock as unknown as typeof fetch, sessions, requests };
}

async function readBody(body: Blob, turns: number): Promise<ArrayBuffer> {
  for (let turn = 0; turn < turns; turn++) await nextMacrotask();
  return body.arrayBuffer();
}

function hangUntilAborted(signal: AbortSignal | null | undefined): Promise<Response> {
  return new Promise<Response>((_, reject) => {
    signal?.addEventListener('abort', () => reject(signal.reason), { once: true });
  });
}

export function fakeAbortSignalTimeout(): void {
  vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
    const controller = new AbortController();
    setTimeout(() => controller.abort(new DOMException('timed out', 'TimeoutError')), ms);
    return controller.signal;
  });
}

export function fileOf(bytes: number, name = 'lucidos-backup-myws.age'): File {
  const data = new Uint8Array(bytes);
  for (let i = 0; i < bytes; i++) data[i] = i % 251;
  return new File([data], name, { lastModified: 1 });
}

const realWorkInFlight = new Set<Promise<unknown>>();
const realBlobArrayBuffer = Blob.prototype.arrayBuffer;
const realResponseText = Response.prototype.text;
const realDigest = crypto.subtle.digest;

function holdClockWhile<T>(work: Promise<T>): Promise<T> {
  realWorkInFlight.add(work);
  const done = () => { realWorkInFlight.delete(work); };
  work.then(done, done);
  return work;
}

export function useUploadFakeTimers(): void {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
  vi.spyOn(Blob.prototype, 'arrayBuffer').mockImplementation(function (this: Blob) {
    return holdClockWhile(realBlobArrayBuffer.call(this));
  });
  vi.spyOn(Response.prototype, 'text').mockImplementation(function (this: Response) {
    return holdClockWhile(realResponseText.call(this));
  });
  vi.spyOn(crypto.subtle, 'digest').mockImplementation((algorithm, data) =>
    holdClockWhile(realDigest.call(crypto.subtle, algorithm, data)));
}

function nextMacrotask(): Promise<void> {
  return new Promise((resolve) => (globalThis as unknown as { setImmediate: (f: () => void) => void }).setImmediate(resolve));
}

export async function runUntilSettled(promise: Promise<unknown>, stepMs = 250, maxMs = 600_000): Promise<void> {
  let settled = false;
  promise.then(() => { settled = true; }, () => { settled = true; });
  for (let elapsed = 0; !settled && elapsed < maxMs;) {
    await nextMacrotask();
    if (realWorkInFlight.size > 0) {
      await Promise.allSettled([...realWorkInFlight]);
      continue;
    }
    await vi.advanceTimersByTimeAsync(stepMs);
    elapsed += stepMs;
  }
}
