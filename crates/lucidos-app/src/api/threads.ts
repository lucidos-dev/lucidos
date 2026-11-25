import { API, ApiError, json, mutatingFetch, throwIfNotOk } from './client';
import type { ThreadSection, ThreadInitiator, ThreadAggregate, EventWaitSummary, ThreadComposeState } from '../store/thread-events';
import type { ThreadStatus } from '../generated/thread-lifecycle';
import type { ComposeSelectionOverride } from '../store/composeSelections';
import type { CacheKeepAliveStatus, UnproposedReason, ComposeRestoration, MessageOrigin } from '../generated/thread-event-wire';
import type { AppKind, WidgetSize } from '../store/types';

export type CodingAgentChangeState =
  | { kind: 'none' }
  | { kind: 'unproposed'; reason: UnproposedReason | null }
  | { kind: 'proposed'; requires_restart: boolean };

export interface SuccessorLink {
  thread_id: string;
  title: string | null;
}

export interface ThreadHoldSummary {
  reason: string;
  root_thread_id: string;
  placed_at: string;
  placed_by?: MessageOrigin;
}

export interface ThreadSummary {
  thread_id: string;
  title: string;
  channel: string;
  initiator: ThreadInitiator;
  created_at: string;
  last_activity: string;
  last_user_action?: string;
  last_agent_action?: string;
  message_count: number;
  saved?: boolean;
  pin_folder?: string;
  home?: boolean;
  section: ThreadSection;
  active_children_count: number;
  awaited_children_count?: number;
  awaited_by_parent?: boolean;
  total_children_count: number;
  blocking_descendant_count: number;
  attention_descendant_count: number;
  archivable_sub_thread_count: number;
  is_stopped_child?: boolean;
  hold?: ThreadHoldSummary;
  read_requested?: boolean;
  reply_seen_devices?: string[];
  apply_in_flight?: boolean;
  live_event_wait_count: number;
  live_event_waits?: EventWaitSummary[];
  cache_keep_alive_status?: CacheKeepAliveStatus;
  status: ThreadStatus;
  summary_version: number;
  coding_agent_change_state: CodingAgentChangeState;
  coding_agent_is_external_repo: boolean;
  last_revived_at: string | null;
  parent_thread_id?: string | null;
  parent_thread_title?: string | null;
  successor?: SuccessorLink;
  successor_of?: SuccessorLink;
  trigger_id?: string | null;
  trigger_name?: string | null;
  cc_repo_id?: string | null;
  cc_repo_name?: string | null;
  coding_agent_kind?: 'lucidos' | 'app' | 'external' | null;
  coding_agent_folder?: string | null;
  coding_agent?: 'claude-code' | 'codex' | null;
  coding_agents?: Array<'claude-code' | 'codex'>;
  state: ThreadComposeState;
  compose_text: string;
  compose_images: string[];
  compose_mode?: 'lucidos' | 'claude_code' | null;
  compose_selection?: ComposeSelectionOverride | null;
  compose_epoch?: number;
  compose_restorations?: ComposeRestoration[];
}

export interface ThreadsResponse {
    saved: ThreadSummary[];
    archive: ThreadSummary[];
    archive_count?: number;
    active: string[];
    active_threads: ThreadSummary[];
    composing: ThreadSummary[];
    family_threads: ThreadSummary[];
    focused_thread?: ThreadSummary;
    home_thread?: ThreadSummary | null;
}

export async function fetchThreads(focusedThreadId?: string): Promise<ThreadsResponse> {
    const params = focusedThreadId ? `?focused=${encodeURIComponent(focusedThreadId)}` : '';
    return json(`${API}/threads${params}`);
}

export async function fetchThreadById(threadId: string): Promise<ThreadSummary | null> {
    try {
        return await json<ThreadSummary>(`${API}/threads/${encodeURIComponent(threadId)}`);
    } catch (err) {
        if (err instanceof ApiError && err.httpCode === 404) return null;
        throw err;
    }
}

async function postThreadAction(path: string, body: Record<string, unknown>): Promise<Response> {
    const res = await mutatingFetch(`${API}/threads/${path}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
    });
    await throwIfNotOk(res);
    return res;
}

export interface RowWriteAck {
  summary_version?: number;
}

async function rowWriteAck(res: Response): Promise<RowWriteAck> {
    const body = await res.text();
    return body ? JSON.parse(body) as RowWriteAck : {};
}

export async function saveThread(threadId: string, folder?: string): Promise<RowWriteAck> {
    const body = folder === undefined ? { thread_id: threadId } : { thread_id: threadId, folder };
    return rowWriteAck(await postThreadAction('save', body));
}

export async function moveThreadToFolder(threadId: string, folder: string | null): Promise<RowWriteAck> {
    return rowWriteAck(await postThreadAction('move-to-folder', { thread_id: threadId, folder }));
}

export async function unsaveThread(threadId: string): Promise<RowWriteAck> {
    return rowWriteAck(await postThreadAction('unsave', { thread_id: threadId }));
}

export interface ArchiveSkippedMember {
  thread_id: string;
  reason: string;
  message: string;
}

export async function archiveThread(
  threadId: string,
): Promise<{ archived: string[]; skipped?: ArchiveSkippedMember[]; summary_versions?: Record<string, number> }> {
    const res = await postThreadAction('archive', { thread_id: threadId });
    return res.json();
}

export interface ArchiveAllPreflight {
  safe: { thread_id: string; title: string }[];
  safe_thread_count: number;
  kept: Record<string, number>;
  kept_thread_count: number;
}

export interface ArchiveAllKept {
  thread_id: string;
  reason: string;
  slug: string;
  thread_count: number;
}

export async function archiveAllPreflight(parentThreadId?: string): Promise<ArchiveAllPreflight> {
    const query = parentThreadId ? `?parent_thread_id=${encodeURIComponent(parentThreadId)}` : '';
    return json<ArchiveAllPreflight>(`${API}/threads/archive-all-preflight${query}`);
}

export async function archiveAll(
  threadIds: string[],
  parentThreadId?: string,
): Promise<{ archived: string[]; kept: ArchiveAllKept[] }> {
    const res = await postThreadAction('archive-all', { thread_ids: threadIds, parent_thread_id: parentThreadId });
    return res.json();
}

export async function unarchiveThreads(
  threadIds: string[],
  { withSubThreads = false }: { withSubThreads?: boolean } = {},
): Promise<{ unarchived: string[] }> {
    const res = await postThreadAction('unarchive', { thread_ids: threadIds, with_sub_threads: withSubThreads });
    return res.json();
}

export interface DeleteBlockingMember {
  thread_id: string;
  title: string | null;
  reason: string;
}

export interface DeletePreflight {
  thread_count: number;
  sub_thread_titles: string[];
  memory_count: number;
  summary_rebuild_count: number;
  has_unapplied_branch_work: boolean;
  has_applied_changes: boolean;
  backups_present: boolean;
  blocked_by: DeleteBlockingMember[];
}

export async function deletePreflight(threadId: string): Promise<DeletePreflight> {
    return json<DeletePreflight>(
        `${API}/threads/delete-preflight?thread_id=${encodeURIComponent(threadId)}`,
    );
}

export async function deleteThreadFamily(
    threadId: string,
): Promise<{ deleted: string[]; event_count: number; memory_count: number }> {
    const res = await postThreadAction('delete', { thread_id: threadId });
    return res.json();
}

export async function detachThread(threadId: string): Promise<void> {
    const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/detach`, {
        method: 'POST',
    });
    await throwIfNotOk(res);
}

export async function markReplySeen(threadId: string, summaryVersion: number): Promise<void> {
    const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/read-request/seen`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ summary_version: summaryVersion }),
    });
    await throwIfNotOk(res);
}

export async function renameThread(threadId: string, title: string): Promise<void> {
    await postThreadAction('rename', { thread_id: threadId, title });
}

export async function suggestTitle(threadId: string): Promise<string> {
    const res = await postThreadAction('suggest-title', { thread_id: threadId });
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
    setDrawerFilterParams(params, sources, triggerIds, repoIds, appIds);
    return json<OlderThreadsResponse>(`${API}/threads/older?${params}`);
}

function setDrawerFilterParams(
  params: URLSearchParams,
  sources?: string[],
  triggerIds?: string[],
  repoIds?: string[],
  appIds?: string[],
): void {
  if (sources && sources.length > 0) params.set('sources', sources.join(','));
  if (triggerIds && triggerIds.length > 0) params.set('trigger_ids', triggerIds.join(','));
  if (repoIds && repoIds.length > 0) params.set('repo_ids', repoIds.join(','));
  if (appIds && appIds.length > 0) params.set('app_ids', appIds.join(','));
}

export async function fetchArchivedCount(
  sources?: string[],
  triggerIds?: string[],
  repoIds?: string[],
  appIds?: string[],
): Promise<number> {
  const params = new URLSearchParams();
  setDrawerFilterParams(params, sources, triggerIds, repoIds, appIds);
  const qs = params.toString();
  const data = await json<{ count: number }>(`${API}/threads/archived-count${qs ? `?${qs}` : ''}`);
  return data.count;
}

export interface FilterFacet {
  id: string | null;
  name: string | null;
  last_activity: string | null;
}

export interface AppFilterFacet {
  id: string;
  name: string | null;
  kind: AppKind | null;
  deleted: boolean;
  last_activity: string | null;
}

export interface FilterFacets {
  triggers: FilterFacet[];
  repos: FilterFacet[];
  apps: AppFilterFacet[];
}

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

export type PairStamp = {
  sequence: number;
  event_type?: string;
  request_event_id?: string | null;
  model: string;
  reasoning_effort: string;
  provider: string;
};

export type CardSize = {
  sequence: number;
  app_id: string;
  params?: Record<string, unknown>;
  size: WidgetSize;
};

export type ThreadEventsSnapshot = {
  events: ThreadEventRow[];
  currentAggregate: ThreadAggregate | null;
  hasMore?: boolean;
  maxSequence?: number;
  pairStamps?: PairStamp[];
  cardSizes?: CardSize[];
};

export interface FetchThreadEventsOptions {
  afterSeq?: number;
  includeContext?: boolean;
  limit?: number;
  before?: { created: string; sequence: number };
}

export async function fetchThreadEvents(
  threadId: string,
  options: FetchThreadEventsOptions | number = {},
): Promise<ThreadEventsSnapshot> {
  const opts: FetchThreadEventsOptions = typeof options === 'number'
    ? { afterSeq: options }
    : options;
  const params = new URLSearchParams();
  if (opts.afterSeq !== undefined) params.set('after', String(opts.afterSeq));
  if (opts.includeContext) params.set('include_context', 'true');
  if (opts.limit !== undefined) params.set('limit', String(opts.limit));
  if (opts.before) {
    params.set('before_created', opts.before.created);
    params.set('before_seq', String(opts.before.sequence));
  }
  const query = params.toString() ? `?${params.toString()}` : '';
  return json(`${API}/threads/${encodeURIComponent(threadId)}/events${query}`);
}

export interface ContextCapturePayload {
  sections: import('../store/types').ContextSection[];
  tools: string[];
}

export async function fetchContextCapture(
  eventId: string,
): Promise<ContextCapturePayload> {
  return json(`${API}/events/${eventId}/context`);
}

export interface ToolResultPayload {
  result: string | null;
}

export async function fetchToolResult(
  eventId: string,
): Promise<ToolResultPayload> {
  return json(`${API}/events/${eventId}/tool-result`);
}

export interface ToolArgsPayload {
  args: unknown;
}

export async function fetchToolArgs(eventId: string): Promise<ToolArgsPayload> {
  return json(`${API}/events/${eventId}/tool-args`);
}

export interface EventLocation {
  thread_id: string | null;
}

export async function fetchEventLocation(eventId: string): Promise<EventLocation> {
  return json(`${API}/events/${eventId}/location`);
}
