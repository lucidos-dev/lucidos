import { API_BASE, ApiError, json, mutatingFetch, throwIfNotOk } from './client';
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
  /** Trigger that fired this thread (only for `channel === 'trigger'`). */
  trigger_id?: string | null;
  /** Trigger name at fire-time (snapshot — falls back when the trigger is renamed/deleted). */
  trigger_name?: string | null;
  /** Repository the CC session bound to (only for `channel === 'claude_code'`). */
  cc_repo_id?: string | null;
  /** Current repo name from the registry — null when the repo was deleted. */
  cc_repo_name?: string | null;
  /** Compose state machine: 'composing' (draft) | 'active' | 'discarded' | 'archived'. */
  state: 'composing' | 'active' | 'discarded' | 'archived';
  /** In-progress compose text. Empty when nothing typed. */
  compose_text: string;
  /** Currently-attached compose image URLs. Empty array when none. */
  compose_images: string[];
  /** User's mode preference while composing. Null once the thread is no longer composing. */
  compose_mode?: 'lucidos' | 'claude_code' | null;
}

export interface ThreadsResponse {
    saved: ThreadInfo[];
    history: ThreadInfo[];
    active: string[];
    active_threads: ThreadInfo[];
    /** Threads in `composing` state — the Drafts surface. Newest-first. */
    composing: ThreadInfo[];
    /** Included when the focused thread isn't in the other lists. */
    focused_thread?: ThreadInfo;
}

export async function fetchThreads(focusedThreadId?: string): Promise<ThreadsResponse> {
    const params = focusedThreadId ? `?focused=${encodeURIComponent(focusedThreadId)}` : '';
    return json(`${API_BASE}/api/threads${params}`);
}

async function postThreadAction(path: string, body: Record<string, unknown>, signal?: AbortSignal): Promise<Response> {
    const res = await mutatingFetch(`${API_BASE}/api/threads/${path}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal,
    });
    await throwIfNotOk(res);
    return res;
}

export async function saveThread(threadId: string): Promise<void> {
    await postThreadAction('save', { thread_id: threadId });
}

export async function unsaveThread(threadId: string): Promise<void> {
    await postThreadAction('unsave', { thread_id: threadId });
}

export async function archiveThread(threadId: string): Promise<void> {
    await postThreadAction('archive', { thread_id: threadId });
}

export async function renameThread(threadId: string, title: string): Promise<void> {
    await postThreadAction('rename', { thread_id: threadId, title });
}

export async function suggestTitle(threadId: string, signal?: AbortSignal): Promise<string> {
    const res = await postThreadAction('suggest-title', { thread_id: threadId }, signal);
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
  triggerIds?: string[],
  repoIds?: string[],
): Promise<OlderThreadsResponse> {
    const params = new URLSearchParams({ before, limit: String(limit) });
    if (sources && sources.length > 0) params.set('sources', sources.join(','));
    if (triggerIds && triggerIds.length > 0) params.set('trigger_ids', triggerIds.join(','));
    if (repoIds && repoIds.length > 0) params.set('repo_ids', repoIds.join(','));
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

import type { ThreadAggregate } from '../store/thread-events';

/** Wraps `events[]` with `currentAggregate` so the historical-replay path
 *  applies meta from a fetched snapshot — same source-of-truth model as live
 *  SSE per-event aggregate. Without it, refresh paths can't reconstruct meta
 *  from events alone (event types no longer carry derivation rules). */
export type ThreadEventsSnapshot = {
  events: ThreadEventRow[];
  currentAggregate: ThreadAggregate | null;
};

export async function fetchThreadEvents(
  threadId: string,
  afterSeq?: number,
): Promise<ThreadEventsSnapshot> {
  const params = afterSeq !== undefined ? `?after=${afterSeq}` : '';
  return json(`${API_BASE}/api/threads/${threadId}/events${params}`);
}
