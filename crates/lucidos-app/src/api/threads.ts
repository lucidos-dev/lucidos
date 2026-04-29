import { API_BASE, ApiError, json } from './client';
import type { ThreadSection, ThreadInitiator } from '../store/thread-events';

export interface ThreadInfo {
  thread_id: string;
  title: string;
  channel: string;
  initiator: ThreadInitiator;
  created_at: string;
  last_activity: string;
  message_count: number;
  section: ThreadSection;
  active_children_count: number;
  total_children_count: number;
  /** Thread status computed by the backend: 'idle', 'running', or 'waiting'. */
  status: string;
  /** Whether the CC session has proposed changes. */
  cc_has_changes: boolean;
  /** Whether the proposed changes require an engine restart. */
  cc_requires_restart: boolean;
  /** Whether the CC session is working on an external repo. */
  cc_is_external_repo: boolean;
  /** Whether a merge conflict is being resolved. */
  cc_applying: boolean;
  /** When the thread last entered 'running' state (ISO string or null). */
  last_revived_at: string | null;
  /** Parent thread that spawned this one (null for user-initiated threads). */
  parent_thread_id?: string | null;
  /** Cached title of the parent thread — null when no parent or no title yet. */
  parent_thread_title?: string | null;
  /** Trigger that started this thread (null for non-trigger threads). */
  trigger_id?: string | null;
}

export interface ThreadsResponse {
    pinned: ThreadInfo[];
    history: ThreadInfo[];
    active: string[];
    active_threads: ThreadInfo[];
    /** Included when the focused thread isn't in the other lists. */
    focused_thread?: ThreadInfo;
}

export async function fetchThreads(focusedThreadId?: string): Promise<ThreadsResponse> {
    const params = focusedThreadId ? `?focused=${encodeURIComponent(focusedThreadId)}` : '';
    return json(`${API_BASE}/api/threads${params}`);
}

export async function pinThread(threadId: string): Promise<void> {
    const res = await fetch(`${API_BASE}/api/threads/pin`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId }),
    });
    if (!res.ok) throw new ApiError(res.status, 'Failed to pin thread');
}

export async function unpinThread(threadId: string): Promise<void> {
    const res = await fetch(`${API_BASE}/api/threads/unpin`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId }),
    });
    if (!res.ok) throw new ApiError(res.status, 'Failed to unpin thread');
}

export async function dismissThread(threadId: string): Promise<void> {
    const res = await fetch(`${API_BASE}/api/threads/dismiss`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId }),
    });
    if (!res.ok) throw new ApiError(res.status, 'Failed to dismiss thread');
}

export async function renameThread(threadId: string, title: string): Promise<void> {
    const res = await fetch(`${API_BASE}/api/threads/rename`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId, title }),
    });
    if (!res.ok) throw new ApiError(res.status, 'Failed to rename thread');
}

export async function suggestTitle(threadId: string, signal?: AbortSignal): Promise<string> {
    const res = await fetch(`${API_BASE}/api/threads/suggest-title`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId }),
        signal,
    });
    if (!res.ok) throw new ApiError(res.status, 'Failed to suggest title');
    const data = await res.json();
    return data.title;
}

export interface ThreadSearchResult extends ThreadInfo {
  score: number;
}

export async function searchThreads(query: string, signal?: AbortSignal): Promise<ThreadSearchResult[]> {
  const res = await fetch(`${API_BASE}/api/threads/search?q=${encodeURIComponent(query)}`, { signal });
  if (!res.ok) throw new ApiError(res.status, 'Failed to search threads');
  const data = await res.json();
  return data.results;
}

export interface OlderThreadsResponse {
    threads: ThreadInfo[];
    has_more: boolean;
}

export async function fetchOlderThreads(
  before: string,
  limit = 15,
  sources?: string[],
): Promise<OlderThreadsResponse> {
    const params = new URLSearchParams({ before, limit: String(limit) });
    if (sources && sources.length > 0) params.set('sources', sources.join(','));
    const res = await fetch(`${API_BASE}/api/threads/older?${params}`);
    if (!res.ok) throw new ApiError(res.status, 'Failed to fetch older threads');
    return res.json();
}

export type ThreadEventRow = {
  sequence: number;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  event_id: string;
};

export async function fetchThreadEvents(
  threadId: string,
  afterSeq?: number,
): Promise<ThreadEventRow[]> {
  const params = afterSeq !== undefined ? `?after=${afterSeq}` : '';
  return json(`${API_BASE}/api/threads/${threadId}/events${params}`);
}
