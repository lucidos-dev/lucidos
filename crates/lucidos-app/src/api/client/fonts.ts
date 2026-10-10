import { API, json, mutatingFetch, throwIfNotOk } from './_core';
import { encodePathSegments } from './data';
import { BASE_PATH } from '../../utils/basePath';

/** A workspace font that failed the engine's checks, and why. */
export interface InvalidWorkspaceFont {
  id: string;
  reason: string;
}

/** `GET /api/v1/fonts`: the font catalog, then the workspace fonts. Only the
 *  workspace half is typed loosely here, because every client re-checks it
 *  (`sanitizeWorkspaceFonts`). */
export interface FontListing {
  fonts: Array<{ source?: string } & Record<string, unknown>>;
  invalid: InvalidWorkspaceFont[];
}

export async function listFonts(): Promise<FontListing> {
  return json<FontListing>(`${API}/fonts`);
}

/** A workspace `data/` file on the static mount, which the shell reaches with
 *  its device credential. `path` must already be URL-safe. */
export function dataMountUrl(path: string): string {
  return `${BASE_PATH}/data/${path}`;
}

/** Write one file of a workspace font (`PUT /api/v1/data/fonts/<slug>/<file>`).
 *  The engine checks it before it touches disk and answers 422 with a reason. */
export async function writeFontFile(path: string, body: Blob | string): Promise<void> {
  const res = await mutatingFetch(`${API}/data/${encodePathSegments(path)}`, {
    method: 'PUT',
    headers: { 'Content-Type': typeof body === 'string' ? 'application/json' : 'application/octet-stream' },
    body,
  });
  await throwIfNotOk(res);
}

/** Every file under `data/fonts/`, as `data/`-relative paths. */
export async function listFontFiles(): Promise<string[]> {
  return json<string[]>(`${API}/data?pattern=${encodeURIComponent('fonts/**')}`);
}

/** Delete one file of a workspace font. */
export async function deleteFontFile(path: string): Promise<void> {
  const res = await mutatingFetch(`${API}/data/${encodePathSegments(path)}`, { method: 'DELETE' });
  await throwIfNotOk(res);
}
