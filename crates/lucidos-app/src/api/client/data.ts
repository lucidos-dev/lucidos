import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface SaveDataFileResult {
  success: boolean;
  path: string;
  commit?: string;
}

export function encodePathSegments(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}

export async function saveDataFile(path: string, content: string): Promise<SaveDataFileResult> {
  const res = await mutatingFetch(`${API}/data/${encodePathSegments(path)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'text/plain' },
    body: content,
  });
  await throwIfNotOk(res);
  return res.json();
}

export interface HandshakeScriptState {
  path: string;
  exists: boolean;
  approved: boolean;
}

export async function fetchHandshakeScripts(): Promise<HandshakeScriptState[]> {
  const body = await json<{ scripts?: HandshakeScriptState[] }>(`${API}/handshake-scripts`);
  return body.scripts ?? [];
}
