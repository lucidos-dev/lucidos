import { fetchWithDeadline, throwIfNotOk } from './_core';
import { sendWithUploadProgress, UPLOAD_RESPONSE_MS, type UploadProgress } from './uploadProgress';

export const TUS_VERSION = '1.0.0';
export const FIRST_CHUNK_BYTES = 32 * 1024 * 1024;
export const FLOOR_CHUNK_BYTES = 512 * 1024;
const HALVINGS_TO_FLOOR = Math.log2(FIRST_CHUNK_BYTES / FLOOR_CHUNK_BYTES);
export const RETRY_DELAYS_MS: readonly number[] = Array.from({ length: HALVINGS_TO_FLOOR }, (_, i) => 1_000 * 2 ** i);

const DISCARD_ATTEMPTS = 3;
const TUS_HEADERS = { 'Tus-Resumable': TUS_VERSION };
const TRANSIENT_STATUSES = new Set([502, 503, 504]);
const CHECKSUM_MISMATCH = 460;
const SESSION_GONE = new Set([404, 410]);
const RESUME_KEY_PREFIX = 'lucidos.restore-upload:';

export interface ResumableUpload {
  base: string;
  file: File;
  signal?: AbortSignal;
  onProgress?: (progress: UploadProgress) => void;
  onRetry?: (attempt: number, reason: unknown) => void;
}

type ResumedSession =
  | { id: string; offset: number }
  | { id: string; offset: null; failure: unknown };

export class ProxyCapError extends Error {
  constructor() {
    super(
      `A proxy between you and Lucidos refuses even ${FLOOR_CHUNK_BYTES / 1024} KiB requests (HTTP 413). `
      + 'Raise its request body limit to restore through it.',
    );
    this.name = 'ProxyCapError';
  }
}

export async function uploadResumable(upload: ResumableUpload): Promise<string> {
  const { base, file, signal } = upload;
  const resumed = await resumeSession(upload);
  const id = resumed?.id ?? (await createSession(base, file, signal));
  rememberSession(base, file, id);
  const url = `${base}/${id}`;
  let offset = 0;
  let chunkBytes = FIRST_CHUNK_BYTES;
  let failures = 0;
  const report = (sentBytes: number) => upload.onProgress?.({ sentBytes, totalBytes: file.size });

  const retry = async (err: unknown) => {
    if (failures >= RETRY_DELAYS_MS.length) throw err;
    upload.onRetry?.(failures + 1, err);
    await sleep(RETRY_DELAYS_MS[failures], signal);
    failures += 1;
    chunkBytes = Math.max(FLOOR_CHUNK_BYTES, Math.floor(chunkBytes / 2));
    try {
      offset = await headOffset(url, signal);
    } catch (headErr) {
      if (signal?.aborted) throw headErr;
    }
  };

  if (resumed?.offset === null) await retry(resumed.failure);
  else offset = resumed?.offset ?? 0;
  report(offset);

  while (offset < file.size) {
    const chunk = file.slice(offset, Math.min(offset + chunkBytes, file.size));
    const headers: Record<string, string> = {
      ...TUS_HEADERS,
      'Content-Type': 'application/offset+octet-stream',
      'Upload-Offset': String(offset),
    };
    const checksum = await sha256Base64(chunk);
    if (checksum) headers['Upload-Checksum'] = `sha256 ${checksum}`;
    const start = offset;
    let res: Response;
    try {
      res = await sendWithUploadProgress(
        { method: 'PATCH', url, headers, body: chunk },
        { signal, onProgress: (p) => report(start + p.sentBytes) },
      );
    } catch (err) {
      if (signal?.aborted) throw err;
      await retry(err);
      continue;
    }
    if (res.status === 204) {
      offset = Number(res.headers.get('Upload-Offset') ?? start + chunk.size);
      failures = 0;
      report(offset);
    } else if (res.status === 413) {
      if (chunkBytes <= FLOOR_CHUNK_BYTES) throw new ProxyCapError();
      chunkBytes = Math.max(FLOOR_CHUNK_BYTES, Math.floor(chunkBytes / 2));
    } else if (isRetryable(res.status)) {
      await retry(await refusal(res));
    } else {
      if (res.status === 404) forgetResumableUpload(base, file);
      await throwIfNotOk(res);
    }
  }
  return id;
}

export function forgetResumableUpload(base: string, file: File): void {
  localStorage.removeItem(resumeKey(base, file));
}

export async function discardResumableUpload(base: string, file: File): Promise<void> {
  const id = localStorage.getItem(resumeKey(base, file));
  if (!id) return;
  for (let attempt = 1; ; attempt++) {
    const res = await fetchWithDeadline(`${base}/${id}`, { method: 'DELETE', headers: TUS_HEADERS }, UPLOAD_RESPONSE_MS);
    if (res.ok || SESSION_GONE.has(res.status)) {
      forgetResumableUpload(base, file);
      return;
    }
    if (attempt >= DISCARD_ATTEMPTS || !isRetryable(res.status)) throw await refusal(res);
    await sleep(RETRY_DELAYS_MS[attempt - 1]);
  }
}

async function resumeSession({ base, file, signal }: ResumableUpload): Promise<ResumedSession | null> {
  const id = localStorage.getItem(resumeKey(base, file));
  if (!id) return null;
  let res: Response;
  try {
    res = await fetchWithDeadline(`${base}/${id}`, { method: 'HEAD', headers: TUS_HEADERS, signal }, UPLOAD_RESPONSE_MS);
  } catch (err) {
    if (signal?.aborted) throw err;
    return { id, offset: null, failure: err };
  }
  if (res.ok && Number(res.headers.get('Upload-Length')) === file.size) {
    return { id, offset: Number(res.headers.get('Upload-Offset') ?? 0) };
  }
  if (res.ok || SESSION_GONE.has(res.status)) {
    forgetResumableUpload(base, file);
    return null;
  }
  if (isRetryable(res.status)) return { id, offset: null, failure: await refusal(res) };
  await throwIfNotOk(res);
  return null;
}

async function createSession(base: string, file: File, signal?: AbortSignal): Promise<string> {
  const res = await fetchWithDeadline(base, {
    method: 'POST',
    headers: {
      ...TUS_HEADERS,
      'Upload-Length': String(file.size),
      'Upload-Metadata': `filename ${base64Utf8(file.name)}`,
    },
    signal,
  }, UPLOAD_RESPONSE_MS);
  await throwIfNotOk(res);
  const location = res.headers.get('Location') ?? '';
  const id = location.split('/').pop();
  if (!id) throw new Error('Lucidos started the upload but did not say where to send it');
  return id;
}

async function headOffset(url: string, signal?: AbortSignal): Promise<number> {
  const res = await fetchWithDeadline(url, { method: 'HEAD', headers: TUS_HEADERS, signal }, UPLOAD_RESPONSE_MS);
  await throwIfNotOk(res);
  return Number(res.headers.get('Upload-Offset') ?? 0);
}

function isRetryable(status: number): boolean {
  return status === 409 || status === CHECKSUM_MISMATCH || TRANSIENT_STATUSES.has(status);
}

async function refusal(res: Response): Promise<unknown> {
  return throwIfNotOk(res).then(() => new Error(`HTTP ${res.status}`), (err: unknown) => err);
}

function rememberSession(base: string, file: File, id: string): void {
  localStorage.setItem(resumeKey(base, file), id);
}

function resumeKey(base: string, file: File): string {
  return `${RESUME_KEY_PREFIX}${base}:${file.name}:${file.size}:${file.lastModified}`;
}

async function sha256Base64(blob: Blob): Promise<string | null> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return null;
  const digest = new Uint8Array(await subtle.digest('SHA-256', await blob.arrayBuffer()));
  return btoa(String.fromCharCode(...digest));
}

function base64Utf8(text: string): string {
  return btoa(String.fromCharCode(...new TextEncoder().encode(text)));
}

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    signal?.throwIfAborted();
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    function onAbort() {
      clearTimeout(timer);
      reject(new DOMException('Upload cancelled', 'AbortError'));
    }
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}
