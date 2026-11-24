import { API, json, mutatingFetch, throwIfNotOk } from './_core';


export interface FrontendPreviewStatus {
  running: boolean;
  thread_id?: string;
  port?: number;
  started_at?: string;
  worktree?: string;
  loopback_only?: boolean;
  url?: string;
}

export function getFrontendPreview(): Promise<FrontendPreviewStatus> {
  return json<FrontendPreviewStatus>(`${API}/frontend-preview`);
}

export async function startFrontendPreview(threadId: string): Promise<FrontendPreviewStatus> {
  const res = await mutatingFetch(`${API}/frontend-preview/start`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ thread_id: threadId }),
  });
  await throwIfNotOk(res);
  return res.json();
}

export async function stopFrontendPreview(): Promise<FrontendPreviewStatus> {
  const res = await mutatingFetch(`${API}/frontend-preview/stop`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({}),
  });
  await throwIfNotOk(res);
  return res.json();
}
