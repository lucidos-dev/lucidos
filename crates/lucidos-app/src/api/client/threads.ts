import { sha256Hex } from '../../utils/sha256Hex';
import { API, json, mutatingFetch, mutatingFetchIdempotent, throwIfNotOk } from './_core';
import { postWithUploadProgress, type UploadObserver } from './uploadProgress';
import type { PreservedComposeIntent } from '../../store/restoreGenerationRecords';
import type { ComposeSelectionOverride } from '../../store/composeSelections';
import { parseComposeReport, type ParsedComposeReport, type ComposeReportWire } from '../../store/composeReport';


export async function ensureThreadStarted(id: string, mode: string): Promise<ParsedComposeReport | null> {
  const res = await mutatingFetchIdempotent(`${API}/threads`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id, mode }),
    timeoutMs: THREAD_START_TIMEOUT_MS,
  });
  await throwIfNotOk(res);
  return readComposeReport(res);
}

export async function createRecoveredDraft(
  id: string,
  generation: string,
  source: PreservedComposeIntent['drafts'][number],
): Promise<void> {
  const hashes = [...new Set(source.draft.image_hashes)];
  const images = new Map<string, { hash: string; base64: string }>();
  const addImage = async (bytes: ArrayBuffer, base64: string, expected?: string) => {
    const hash = await sha256Hex(bytes);
    if (!hash) throw new Error('Image recovery requires secure image hashing');
    if (expected && hash !== expected) throw new Error('Recovered image hash mismatch');
    if (!hashes.includes(hash)) hashes.push(hash);
    images.set(hash, { hash, base64 });
  };
  for (const upload of source.uploads ?? []) {
    const bytes = new Uint8Array(upload.bytes);
    let binary = '';
    for (let offset = 0; offset < bytes.length; offset += 8192) {
      binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
    }
    await addImage(upload.bytes, btoa(binary), upload.landedHash);
  }
  for (const image of source.unsentMessage?.body.images ?? []) {
    const bytes = Uint8Array.from(atob(image.base64), (c) => c.charCodeAt(0));
    await addImage(bytes.buffer, image.base64);
  }
  const res = await mutatingFetch(`${API}/threads`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id, mode: source.draft.mode ?? 'lucidos',
      ...(images.size ? { recovered_images: [...images.values()] } : {}), recovered_compose: {
      generation, text: source.draft.text, image_hashes: hashes,
      selection: source.selection,
    } }),
    timeoutMs: THREAD_START_TIMEOUT_MS,
  }, true);
  await throwIfNotOk(res);
}

async function readComposeReport(res: Response): Promise<ParsedComposeReport | null> {
  const body = await res.json().catch(() => null) as ComposeReportWire | null;
  return typeof body?.summary_version === 'number' ? parseComposeReport(body) : null;
}

export const THREAD_START_TIMEOUT_MS = 15_000;

export type ComposePutResult =
  | { status: 'applied'; report: ParsedComposeReport | null }
  | { status: 'stale'; report: ParsedComposeReport };

export async function putComposeOnThread(
  threadId: string,
  text: string,
  imageHashes: string[] | null,
  mode: string | null,
  selection?: ComposeSelectionOverride,
  composeEpoch?: number,
): Promise<ComposePutResult> {
  const res = await mutatingFetchIdempotent(
    `${API}/threads/${encodeURIComponent(threadId)}/compose`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        text,
        image_hashes: imageHashes,
        mode,
        selection,
        compose_epoch: composeEpoch,
      }),
    },
  );
  if (res.status === 412) {
    const body = await res.clone().json().catch(() => null) as ComposeReportWire | null;
    if (typeof body?.compose_epoch === 'number') return { status: 'stale', report: parseComposeReport(body) };
  }
  await throwIfNotOk(res);
  return { status: 'applied', report: await readComposeReport(res) };
}

export interface BlobUploadResponse {
  hash: string;
  mime: string;
  byte_size: number;
}

export async function uploadThreadBlob(
  threadId: string,
  file: File,
  observer?: UploadObserver,
): Promise<BlobUploadResponse> {
  const form = new FormData();
  form.append('file', file);
  const res = await postWithUploadProgress(
    `${API}/threads/${encodeURIComponent(threadId)}/blobs`,
    form,
    observer,
  );
  await throwIfNotOk(res);
  return (await res.json()) as BlobUploadResponse;
}

export function blobPreviewUrl(hash: string): string {
  return `${API}/blobs/${encodeURIComponent(hash)}/preview`;
}

export async function deleteThread(id: string): Promise<number | null> {
  const res = await mutatingFetchIdempotent(
    `${API}/threads/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  );
  await throwIfNotOk(res);
  const body = await res.json().catch(() => null) as { summary_version?: unknown } | null;
  return typeof body?.summary_version === 'number' ? body.summary_version : null;
}

export type SearchCategory = 'all' | 'threads' | 'files' | 'apps' | 'triggers' | 'settings' | 'changes' | 'text' | 'menu';

export type ServerSearchCategory = Exclude<SearchCategory, 'settings' | 'menu' | 'text'>;

export interface SearchResultItem {
  id: string;
  title: string;
  subtitle: string;
  category: string;
  score: number;
  last_activity?: string;
}

export interface SearchResults {
  results: Record<string, SearchResultItem[]>;
}

export async function searchEverywhere(
  query: string,
  category: ServerSearchCategory,
  { signal, limit }: { signal?: AbortSignal; limit?: number } = {},
): Promise<SearchResults> {
  const params = new URLSearchParams({ category });
  if (query) params.set('q', query);
  if (limit !== undefined) params.set('limit', String(limit));
  return json<SearchResults>(`${API}/search?${params}`, { signal });
}

export type TextSearchMode = 'preview' | 'all';

export interface TextSearchHit {
  path: string;
  line: number;
  before: string;
  matched: string;
  after: string;
}

export type TextSearchResponse =
  | { status: 'query-too-short'; min_query_chars: number }
  | {
    status: 'ok';
    hits: TextSearchHit[];
    truncated: boolean;
    skipped_large_files: number;
  };

export async function searchText(
  query: string,
  mode: TextSearchMode,
  { signal }: { signal?: AbortSignal } = {},
): Promise<TextSearchResponse> {
  const params = new URLSearchParams({ q: query, mode });
  return json<TextSearchResponse>(`${API}/search/text?${params}`, { signal });
}
