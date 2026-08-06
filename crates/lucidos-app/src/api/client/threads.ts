import { API, json, mutatingFetchIdempotent, throwIfNotOk } from './_core';
import type { ComposeSelectionOverride } from '../../store/composeSelections';

// --- Compose state machine (threads-as-drafts) ---
//
// Compose drafts → POST /threads + PUT /threads/:id/compose; DELETE /threads/:id.
// Followup drafts (compose payload on an active thread) → PUT /threads/:id/compose.
// State-machine guard returns 410 on writes to discarded threads.
// See `docs/plans/2026-05-03-threads-as-drafts-design.md`.

/** Ensure a thread exists in `composing` state. Idempotent on the same
 *  `{id, mode}` (POST /threads returns 200). 409 propagates — a different
 *  mode (Composing) or wrong state (Active/Archived) is a real conflict the
 *  caller must surface; swallowing it would silently overwrite the user's
 *  mode toggle when two devices race the first keystroke. */
export async function ensureThreadStarted(id: string, mode: string): Promise<void> {
  const res = await mutatingFetchIdempotent(`${API}/threads`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id, mode }),
  });
  await throwIfNotOk(res);
}

/** What the engine did with a compose write.
 *
 *  `stale` is NOT an error, which is why it is a return value rather than a
 *  throw: the write was composed against a *compose epoch* a submission has
 *  since consumed, so the engine dropped it and handed back the current epoch.
 *  The caller adopts that value and re-issues. Throwing would route it into the
 *  "the engine refused, tell the user" branch, and the user has nothing to fix. */
export type ComposePutResult =
  | { status: 'applied' }
  | { status: 'stale'; composeEpoch: number };

/** PUT /api/v1/threads/:id/compose. `image_hashes` semantics mirror the
 *  SQL COALESCE on the backend: `null` preserves, `[]` clears, `[h,…]`
 *  replaces. Hashes come from prior `uploadThreadBlob` calls. `selection`
 *  follows the same COALESCE-preserve rule: `undefined` (omitted) preserves the
 *  stored per-draft dropdown selection (a text-only keystroke PUT never wipes
 *  the draft's picks), while an object replaces it.
 *
 *  `composeEpoch` is the write fence: the epoch this draft was composed
 *  against. Omitting it asks the engine to apply the write unfenced, which only
 *  a client that predates the epoch should do. */
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
      // Omit `selection` when undefined so the backend COALESCE preserves the
      // stored value (a bare keystroke PUT must not clobber the picks).
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
    // The body carries the epoch the engine now holds. A malformed one would
    // leave the caller unable to make progress, so fall through to the throw
    // rather than guessing an epoch: a refusal the caller cannot act on is at
    // least a refusal the user is told about.
    const body = await res.json().catch(() => null);
    const epoch = (body as { compose_epoch?: unknown } | null)?.compose_epoch;
    if (typeof epoch === 'number') return { status: 'stale', composeEpoch: epoch };
  }
  await throwIfNotOk(res);
  return { status: 'applied' };
}

/** Response shape from `POST /api/v1/threads/:id/blobs`. */
export interface BlobUploadResponse {
  hash: string;
  mime: string;
  byte_size: number;
}

/** Upload an image and get its content-addressed sha256 hash. Idempotent
 *  on the bytes (same upload twice = same hash, no extra disk write), so
 *  retries are safe. */
export async function uploadThreadBlob(
  threadId: string,
  file: File,
): Promise<BlobUploadResponse> {
  const form = new FormData();
  form.append('file', file);
  const res = await mutatingFetchIdempotent(
    `${API}/threads/${encodeURIComponent(threadId)}/blobs`,
    {
      method: 'POST',
      body: form,
    },
  );
  await throwIfNotOk(res);
  return (await res.json()) as BlobUploadResponse;
}

/** Browser-display URL for a content-addressed blob: a downscaled JPEG
 *  (long edge ≤ 2048 px, quality 85). Use for `<img>` tags — the original
 *  is multi-megapixel iPhone JPEG that iOS Safari/WebKit can't decode at
 *  fullscreen size without exceeding its per-image memory budget (renders
 *  fully black). Engine generates the preview on first request and caches
 *  it under `.lucidos/blob-previews/`; same `Cache-Control: immutable,
 *  max-age=1y` as the originals so once fetched the browser never
 *  re-requests it. The original full-resolution bytes remain available at
 *  `/api/v1/blobs/{hash}` (no frontend caller today; reserved for a
 *  future "download original" affordance). */
export function blobPreviewUrl(hash: string): string {
  return `${API}/blobs/${encodeURIComponent(hash)}/preview`;
}

/** Discard a composing thread. 410 (already discarded) and 404 (never
 *  existed) are the desired end-state — caller swallows. 409 (Active /
 *  Archived) is a real conflict — caller surfaces. */
export async function deleteThread(id: string): Promise<void> {
  const res = await mutatingFetchIdempotent(
    `${API}/threads/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  );
  await throwIfNotOk(res);
}

// --- Search Everywhere ---
export type SearchCategory = 'all' | 'threads' | 'files' | 'apps' | 'triggers' | 'settings' | 'changes';

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

export async function searchEverywhere(query: string, category: SearchCategory, signal?: AbortSignal): Promise<SearchResults> {
  const params = new URLSearchParams({ category });
  if (query) params.set('q', query);
  return json<SearchResults>(`${API}/search?${params}`, { signal });
}
