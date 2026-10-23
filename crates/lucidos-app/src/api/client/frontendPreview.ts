import { API, json, mutatingFetch, throwIfNotOk } from './_core';

// --- Frontend preview (dev): the supervised Vite dev server showing a
// --- coding-agent worktree's frontend before Apply.

/**
 * What the engine reports about the one preview slot.
 *
 * `url` is computed by the engine from the `Host` of the request that asked, so
 * a phone gets a Tailscale URL and a laptop gets localhost. It is absent when
 * nothing is running, and absent for a caller that sent no `Host`.
 */
export interface FrontendPreviewStatus {
  running: boolean;
  thread_id?: string;
  port?: number;
  started_at?: string;
  worktree?: string;
  url?: string;
}

export function getFrontendPreview(): Promise<FrontendPreviewStatus> {
  return json<FrontendPreviewStatus>(`${API}/frontend-preview`);
}

/** Start (or move) the preview onto this thread's worktree. Replaces whatever
 *  was running: there is one slot per workspace. */
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
