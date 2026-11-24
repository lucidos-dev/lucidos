import { API, json, mutatingFetch, throwIfNotOk } from './_core';
import { encodePathSegments } from './data';
import { BASE_PATH } from '../../utils/basePath';

export interface InvalidWorkspaceFont {
  id: string;
  reason: string;
}

export interface FontListing {
  fonts: Array<{ source?: string } & Record<string, unknown>>;
  invalid: InvalidWorkspaceFont[];
}

export async function listFonts(): Promise<FontListing> {
  return json<FontListing>(`${API}/fonts`);
}

export function dataMountUrl(path: string): string {
  return `${BASE_PATH}/data/${path}`;
}

export async function writeFontFile(path: string, body: Blob | string): Promise<void> {
  const res = await mutatingFetch(`${API}/data/${encodePathSegments(path)}`, {
    method: 'PUT',
    headers: { 'Content-Type': typeof body === 'string' ? 'application/json' : 'application/octet-stream' },
    body,
  });
  await throwIfNotOk(res);
}

export async function listFontFiles(): Promise<string[]> {
  return json<string[]>(`${API}/data?pattern=${encodeURIComponent('fonts/**')}`);
}

export async function deleteFontFile(path: string): Promise<void> {
  const res = await mutatingFetch(`${API}/data/${encodePathSegments(path)}`, { method: 'DELETE' });
  await throwIfNotOk(res);
}
