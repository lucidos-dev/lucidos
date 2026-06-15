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
  /** 'idle' | 'running' | 'waiting' | 'failed' | 'waiting_for_user_answer'.
   *  Active = 'running' | 'waiting_for_user_answer' (the agentic loop is
   *  mid-flow). `waiting` is NOT active — it means the coding agent has stopped and
   *  proposed changes the user must act on; the loop has paused. */
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
  /** When true, return only threads whose `status` indicates the agentic
   *  loop is still running (`running` / `waiting_for_user_answer`).
   *  `waiting` is NOT active — it means the coding agent has stopped and proposed
   *  changes the user must act on; the loop has paused. */
  active?: boolean;
  /** Comma-separated source filter (`chat`, `trigger`, `coding-agent`).
   *  Legacy `claude_code` is also accepted. */
  source?: string;
  /** Server clamps to 1..=1000 (default 100). */
  limit?: number;
}

function buildListQuery(opts?: ThreadsListOptions): string {
  if (!opts) return '';
  const params = new URLSearchParams();
  if (opts.active !== undefined) params.set('active', String(opts.active));
  if (opts.source) params.set('source', opts.source);
  if (opts.limit !== undefined) params.set('limit', String(opts.limit));
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
