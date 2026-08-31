import { API, json, mutatingFetch, throwIfNotOk } from './_core';

export interface SaveDataFileResult {
  success: boolean;
  path: string;
  commit?: string;
}

/** Encode each path segment individually, preserving `/` separators — matches
 *  the SDK's `lucidos.data` encoding so a path like `artifacts/my notes/x.md`
 *  resolves to the same URL the engine serves. */
function encodePathSegments(path: string): string {
  return path.split('/').map(encodeURIComponent).join('/');
}

/** Overwrite a workspace data file with new text content
 *  (PUT /api/v1/data/*path). Sends the device-id header via `mutatingFetch` so
 *  the engine attributes the resulting `DataFileWritten` event to this device.
 *  Rejected by the engine for read-only prefixes (e.g. `system-knowhow/`). */
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
  /** Workspace-relative, e.g. `data/scripts/auth/comfort-cloud.py`. */
  path: string;
  exists: boolean;
  approved: boolean;
}

/** Which auth handshake scripts may run (GET /api/v1/handshake-scripts).
 *  Read-only: approving one is refused to a browser and belongs to
 *  `lucidos handshake approve` (ADR 0144). */
export async function fetchHandshakeScripts(): Promise<HandshakeScriptState[]> {
  const body = await json<{ scripts?: HandshakeScriptState[] }>(`${API}/handshake-scripts`);
  return body.scripts ?? [];
}
