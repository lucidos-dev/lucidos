import { request } from './_fetch';
import { LIVENESS_STATES } from './generated/engine-constants';
import { people, users } from './threadUsers';

/** What a thread is doing now, as the engine reports it (ADR 0679). The
 *  engine derives it; a client only shows it. */
export type LivenessState = (typeof LIVENESS_STATES)[number];

/** A thread's *thread liveness*: the state plus the evidence behind it.
 *  `as_of` is the engine's clock when it looked. */
export interface ThreadLiveness {
  state: LivenessState;
  status: string;
  last_activity_age_secs?: number;
  /** Absent on a thread with no agent session, such as a chat thread. */
  session?: {
    ordinal: number;
    continuity: 'first' | 'fresh' | 'handoff';
    age_secs: number;
  };
  /** Present only while a handoff waits for its successor session. */
  handoff?: 'pending';
  as_of: string;
}

/** Why a turn end left a coding agent's work unproposed. Mirrors the engine's
 *  `UnproposedReason` wire names. */
export type UnproposedReason =
  | 'plan_missing'
  | 'plan_awaiting_approval'
  | 'plan_only_code'
  | 'outside_bound'
  | 'hardening_missing'
  | 'turn_incomplete'
  | 'awaiting_landing'
  | 'integrated_into_parent'
  | 'carried_by_successor';

/** What a coding-agent thread's branch holds. Mirrors the engine wire (ADR
 *  0400). `none`: no work. `unproposed`: work with no pending change, and
 *  `reason` says why a turn end withheld it. `proposed`: a pending change the
 *  user can apply. */
export type CodingAgentChangeState =
  | { kind: 'none' }
  | { kind: 'unproposed'; reason: UnproposedReason | null }
  | { kind: 'proposed'; requires_restart: boolean };

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
  coding_agent_change_state: CodingAgentChangeState;
  coding_agent_is_external_repo: boolean;
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
  /** Filled on `list()` rows. */
  liveness?: ThreadLiveness;
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

  /** One thread's liveness. Read it before saying a thread is stuck. */
  liveness(threadId: string): Promise<ThreadLiveness> {
    return request<ThreadLiveness>(`/threads/${encodeURIComponent(threadId)}/liveness`);
  },

  /** Driver and watchers of a root thread (ADR 0623). */
  users,

  /** @deprecated Use `threads.users`. Kept for apps written before the user
   *  rename (docs/temporary-measures.md, "SDK threadPeople alias"). */
  people,
};
