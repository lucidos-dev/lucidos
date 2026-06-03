import { API, json, mutatingFetch, throwIfNotOk } from './client';
import type { ThreadSection, ThreadInitiator, ThreadAggregate } from '../store/thread-events';

export interface ThreadSummary {
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
  /** Count of descendants (transitive) currently in a state that blocks this
   *  thread from being archived. Maintained by EventBus on
   *  `thread_summaries.blocking_descendant_count`. Consumed by
   *  `resolveActions` via `count > 0`; the raw count enables UI like
   *  "3 sub-threads still busy". */
  blocking_descendant_count: number;
  /** Count of descendants (transitive) currently in a state that needs user
   *  attention (WaitingForUserAnswer, or an in-workspace CC thread with
   *  pending changes). Strict subset of `blocking_descendant_count` — drops
   *  the Running case. Consumed by `displaySection` via `count > 0` to bubble
   *  the parent to REVIEW even when sibling descendants are still running. */
  attention_descendant_count: number;
  /** Thread status computed by the backend: 'idle', 'running', or 'waiting'. */
  status: string;
  /** Whether the CC branch has any diff against main on disk — pure git
   *  truth. Backs the WaitingBanner Diff button independently of the
   *  proposal lifecycle. */
  coding_agent_has_diff: boolean;
  /** CC's formal "ready for review" — set by `ChangeProposed`. Backs the
   *  Apply / Discard buttons. */
  coding_agent_proposed: boolean;
  /** Whether the proposed change requires an engine restart. Only meaningful
   *  when `coding_agent_proposed` is true. */
  coding_agent_requires_restart: boolean;
  /** Whether the Claude Code session is working on an external repo. */
  coding_agent_is_external_repo: boolean;
  /** Whether a merge conflict is being resolved. */
  coding_agent_applying: boolean;
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
  /** Repository the Claude Code session bound to (only for `channel === 'claude_code'`). */
  cc_repo_id?: string | null;
  /** Current repo name from the registry — null when the repo was deleted. */
  cc_repo_name?: string | null;
  /** Coding-agent thread flavor: 'lucidos' | 'app' | 'external'. Drives
   *  app-specific affordances like the WIP preview button and the app-icon
   *  branch chip. Null for non-CC threads and legacy CC rows (consumers
   *  default null → 'lucidos'). */
  coding_agent_kind?: 'lucidos' | 'app' | 'external' | null;
  /** Canonical folder the coding agent operates on. For `coding_agent_kind ===
   *  'app'`, the last segment is the app id (`<ws>/data/apps/<id>/`). Null
   *  for non-CC threads and legacy rows. */
  coding_agent_folder?: string | null;
  /** Compose state machine: 'composing' (draft) | 'active' | 'discarded'. The
   *  archive flag lives on the separate `archive_state` field — an archived
   *  thread carries `state='active'` plus `archive_state='archived'`. */
  state: 'composing' | 'active' | 'discarded';
  /** In-progress compose text. Empty when nothing typed. */
  compose_text: string;
  /** Currently-attached compose image URLs. Empty array when none. */
  compose_images: string[];
  /** User's mode preference while composing. Null once the thread is no longer composing. */
  compose_mode?: 'lucidos' | 'claude_code' | null;
}

export interface ThreadsResponse {
    saved: ThreadSummary[];
    archive: ThreadSummary[];
    active: string[];
    active_threads: ThreadSummary[];
    /** Threads in `composing` state — the Drafts surface. Newest-first. */
    composing: ThreadSummary[];
    /** Ancestors + descendants of the other lists that aren't already in them.
     *  Loaded eagerly so the drawer's family-aware sort can nest every child
     *  under its parent — see `ThreadDrawer.tsx → categorizeThreads`. These
     *  threads MUST NOT advance the pagination cursor; treat them as
     *  display-only fill-in. */
    family_threads: ThreadSummary[];
    /** Included when the focused thread isn't in the other lists. */
    focused_thread?: ThreadSummary;
}

export async function fetchThreads(focusedThreadId?: string): Promise<ThreadsResponse> {
    const params = focusedThreadId ? `?focused=${encodeURIComponent(focusedThreadId)}` : '';
    return json(`${API}/threads${params}`);
}

async function postThreadAction(path: string, body: Record<string, unknown>, signal?: AbortSignal): Promise<Response> {
    const res = await mutatingFetch(`${API}/threads/${path}`, {
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

/** Cascading archive — backend archives the target thread and every
 *  descendant in one transaction. Returns the list of thread IDs whose
 *  ThreadArchived event was emitted (excludes descendants that were
 *  already archived). 409 surfaces as ApiError with the engine's
 *  structured reason ("parent_not_archivable" | "descendants_blocking"). */
export async function archiveThread(threadId: string): Promise<{ archived: string[] }> {
    const res = await postThreadAction('archive', { thread_id: threadId });
    return res.json();
}

export async function renameThread(threadId: string, title: string): Promise<void> {
    await postThreadAction('rename', { thread_id: threadId, title });
}

export async function suggestTitle(threadId: string, signal?: AbortSignal): Promise<string> {
    const res = await postThreadAction('suggest-title', { thread_id: threadId }, signal);
    const data = await res.json();
    return data.title;
}

export interface ThreadSearchResult extends ThreadSummary {
  score: number;
}

export async function searchThreads(query: string, signal?: AbortSignal): Promise<ThreadSearchResult[]> {
  const data = await json<{ results: ThreadSearchResult[] }>(
    `${API}/threads/search?q=${encodeURIComponent(query)}`,
    { signal },
  );
  return data.results;
}

export interface OlderThreadsResponse {
    threads: ThreadSummary[];
    /** Family members of `threads` not already in the loaded set. See
     *  `ThreadsResponse.family_threads` — display-only, never drives the
     *  pagination cursor. */
    family_threads: ThreadSummary[];
    has_more: boolean;
}

export async function fetchOlderThreads(
  before: string,
  limit = 15,
  sources?: string[],
  triggerIds?: string[],
  repoIds?: string[],
  appIds?: string[],
): Promise<OlderThreadsResponse> {
    const params = new URLSearchParams({ before, limit: String(limit) });
    if (sources && sources.length > 0) params.set('sources', sources.join(','));
    if (triggerIds && triggerIds.length > 0) params.set('trigger_ids', triggerIds.join(','));
    if (repoIds && repoIds.length > 0) params.set('repo_ids', repoIds.join(','));
    if (appIds && appIds.length > 0) params.set('app_ids', appIds.join(','));
    return json<OlderThreadsResponse>(`${API}/threads/older?${params}`);
}

/** One selectable drawer-filter facet (trigger / repo / app) returned by
 *  `/api/v1/threads/filter-facets`. `id` is the trigger id / repo UUID / app
 *  id; `last_activity` is the newest thread for that facet (ISO8601), used to
 *  order deleted entries. Labels are resolved client-side from the registries. */
export interface FilterFacet {
  id: string | null;
  last_activity: string | null;
}

export interface FilterFacets {
  triggers: FilterFacet[];
  repos: FilterFacet[];
  apps: FilterFacet[];
}

/** Complete set of selectable filter facets — every trigger/repo/app that has
 *  at least one thread, so the drawer "Show" dropdown lists them all regardless
 *  of what's currently loaded. */
export async function fetchFilterFacets(): Promise<FilterFacets> {
    return json<FilterFacets>(`${API}/threads/filter-facets`);
}

export type ThreadEventRow = {
  sequence: number;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  event_id: string;
};

/** Wraps `events[]` with `currentAggregate` so the historical-replay path
 *  applies meta from a fetched snapshot — same source-of-truth model as live
 *  SSE per-event aggregate. Without it, refresh paths can't reconstruct meta
 *  from events alone (event types no longer carry derivation rules). */
export type ThreadEventsSnapshot = {
  events: ThreadEventRow[];
  currentAggregate: ThreadAggregate | null;
};

export interface FetchThreadEventsOptions {
  afterSeq?: number;
  /** Opt the snapshot back into full `ContextCaptured` payloads. Default
   *  false — the strip is on so heavy threads load fast. `exportThread.ts`
   *  passes true so bug-report dumps stay complete. */
  includeContext?: boolean;
}

export async function fetchThreadEvents(
  threadId: string,
  options: FetchThreadEventsOptions | number = {},
): Promise<ThreadEventsSnapshot> {
  // Back-compat: callers passing a bare `afterSeq` number (the historical
  // signature) keep working. New callers pass an options object.
  const opts: FetchThreadEventsOptions = typeof options === 'number'
    ? { afterSeq: options }
    : options;
  const params = new URLSearchParams();
  if (opts.afterSeq !== undefined) params.set('after', String(opts.afterSeq));
  if (opts.includeContext) params.set('include_context', 'true');
  const query = params.toString() ? `?${params.toString()}` : '';
  return json(`${API}/threads/${threadId}/events${query}`);
}

/** Payload returned by the lazy-load endpoint for one `ContextCaptured`
 *  event. The snapshot endpoint strips these to keep the events list small;
 *  the step-detail modal fetches them on open. Mirrors Rust
 *  `ContextCapturePayload` in `api/threads.rs`. */
export interface ContextCapturePayload {
  sections: import('../store/types').ContextSection[];
  tools: string[];
}

/** Lazy-fetch `sections` + `tools` for one `ContextCaptured` event. Keyed on
 *  `eventId` only — the route does not depend on the thread id, since the
 *  modal can't always tell which thread the snap belongs to (it may be open
 *  while focus has navigated elsewhere). */
export async function fetchContextCapture(
  eventId: string,
): Promise<ContextCapturePayload> {
  return json(`${API}/events/${eventId}/context`);
}

/** Payload returned by the lazy-load endpoint for one `ToolResult` event.
 *  The snapshot endpoint strips `result` to keep the events list small;
 *  the step-detail modal fetches it on open. `result` is `null` for
 *  image-only tool results (no textual result was ever written).
 *  Mirrors Rust `ToolResultPayload` in `api/threads.rs`. */
export interface ToolResultPayload {
  result: string | null;
}

/** Lazy-fetch the `result` string for one `ToolResult` event. Same
 *  event-id-only routing as `fetchContextCapture`. */
export async function fetchToolResult(
  eventId: string,
): Promise<ToolResultPayload> {
  return json(`${API}/events/${eventId}/tool-result`);
}
