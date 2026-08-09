import { request } from './_fetch';

/** Projected snapshot of a thread's metadata, derived from the event stream
 *  by the `thread_summaries` projection. Same shape the engine returns from
 *  `GET /api/v1/threads/list`, the `lucidos threads list` CLI, and the
 *  `list_threads` LLM tool — the single canonical "thread summary" surface. */
export interface ThreadSummary {
  thread_id: string;
  title: string;
  channel: string;
  initiator: 'user' | 'system';
  created_at: string;
  last_activity: string;
  message_count: number;
  /** Inbox/archive sectioning, stored in `thread_summaries.archive_state`. */
  section: string;
  active_children_count: number;
  total_children_count: number;
  /** 'idle' | 'running' | 'waiting' | 'paused' | 'failed' |
   *  'waiting_for_user_answer'.
   *  Active is the UNION 'running' | 'waiting_for_user_answer', which groups two
   *  opposite situations: 'running' is the workspace working, while
   *  'waiting_for_user_answer' is the workspace stopped and waiting on a person.
   *  Filter on 'running' alone to ask whether anything is busy. `waiting` is in
   *  neither: it means the coding agent has stopped and proposed changes the
   *  user must act on. Neither is `paused`, which means an engine restart
   *  interrupted the turn: it either resumes on its own or offers the user a
   *  Continue button. */
  status: string;
  coding_agent_has_diff: boolean;
  coding_agent_proposed: boolean;
  coding_agent_requires_restart: boolean;
  coding_agent_is_external_repo: boolean;
  coding_agent_applying: boolean;
  last_revived_at: string | null;
  parent_thread_id?: string | null;
  parent_thread_title?: string | null;
  trigger_id?: string | null;
  trigger_name?: string | null;
  cc_repo_id?: string | null;
  cc_repo_name?: string | null;
  state: 'composing' | 'active' | 'discarded' | 'archived';
  compose_text: string;
  compose_images: string[];
  compose_mode?: 'lucidos' | 'claude_code' | null;
}

export interface ThreadsListOptions {
  /** The UNION of `running` and `waiting_for_user_answer`. `true` selects it,
   *  `false` inverts it, omitting it filters nothing.
   *
   *  For "is the workspace busy?" pass `status: 'running'` instead: a thread
   *  awaiting a user answer is blocked on the human, not working, so it is in
   *  this union while being the opposite of busy. `waiting` is in neither: it
   *  means the coding agent has stopped and proposed changes the user must act
   *  on. Mutually exclusive with `status`. */
  active?: boolean;
  /** Comma-separated status filter naming exactly the statuses to keep, in the
   *  same spelling each returned row's `status` field carries: `idle`,
   *  `running`, `waiting`, `waiting_for_user_answer`, `paused`, `failed`. The
   *  precise form of `active`, and mutually exclusive with it. An unrecognized
   *  or empty value is a 400, never a silently empty or unfiltered result. */
  status?: string;
  /** Comma-separated source filter (`chat`, `trigger`, `coding-agent`).
   *  Legacy `claude_code` is also accepted. */
  source?: string;
  /** Server clamps to 1..=1000 (default 100). */
  limit?: number;
  /** Thread id. Restrict to that thread's DIRECT children only, never its
   *  grandchildren. Same filter the `lucidos threads list --parent` CLI flag
   *  and the `list_threads` LLM tool's `my_children` argument use; the tool
   *  resolves it from its own calling thread, while an app has to name one.
   *  A malformed uuid is a 400, never a silently unfiltered list. */
  parent?: string;
}

function buildListQuery(opts?: ThreadsListOptions): string {
  if (!opts) return '';
  const params = new URLSearchParams();
  if (opts.active !== undefined) params.set('active', String(opts.active));
  // `!== undefined`, not truthiness: an empty `status` is a 400 by contract,
  // and dropping it here would answer "everything" to a caller whose filter
  // came out blank, which is the exact silent broadening this filter exists to
  // prevent. `source` keeps its truthiness check on purpose: a blank `source`
  // collapses to "no filter" server-side (`parse_source_filter`), so there the
  // two agree.
  if (opts.status !== undefined) params.set('status', opts.status);
  if (opts.source) params.set('source', opts.source);
  if (opts.limit !== undefined) params.set('limit', String(opts.limit));
  if (opts.parent) params.set('parent', opts.parent);
  const s = params.toString();
  return s ? `?${s}` : '';
}

export const threads = {
  /** Newest-first list of thread summaries. */
  list(opts?: ThreadsListOptions): Promise<ThreadSummary[]> {
    return request<ThreadSummary[]>(`/threads/list${buildListQuery(opts)}`);
  },

  /** Count threads matching the same filters as `list()`. */
  count(opts?: Omit<ThreadsListOptions, 'limit'>): Promise<number> {
    return request<{ count: number }>(`/threads/count${buildListQuery(opts)}`)
      .then(r => r.count);
  },
};
