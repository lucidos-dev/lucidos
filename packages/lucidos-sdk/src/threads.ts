import { request } from './_fetch';
import { LIVENESS_STATES } from './generated/engine-constants';
import { people, users } from './threadUsers';
import type { EventSubscription } from './triggers';

export type LivenessState = (typeof LIVENESS_STATES)[number];

export interface ThreadLiveness {
  state: LivenessState;
  status: string;
  last_activity_age_secs?: number;
  session?: {
    ordinal: number;
    continuity: 'first' | 'fresh' | 'handoff';
    age_secs: number;
  };
  handoff?: 'pending';
  live_event_waits?: Array<{
    reason: string;
    on: EventSubscription[];
    expires_at: string;
    expires_in_secs: number;
    background_timing?: { state: 'queued' } | { state: 'running'; deadline: string };
  }>;
  awaited_children?: Array<{ thread_id: string; title: string }>;
  expected_wake?: { at: string; source: 'agent' | 'decision'; reason: string };
  hold?: { by: 'stop' } | { by: 'open_card'; card_id: string } | { by: 'hold'; reason: string; root_thread_id: string };
  as_of: string;
}

export type UnproposedReason =
  | 'plan_missing'
  | 'plan_changed'
  | 'plan_unreadable'
  | 'plan_awaiting_approval'
  | 'plan_only_code'
  | 'outside_bound'
  | 'hardening_missing'
  | 'turn_incomplete'
  | 'awaiting_landing'
  | 'integrated_into_parent'
  | 'carried_by_successor';

export type CodingAgentChangeState =
  | { kind: 'none' }
  | { kind: 'unproposed'; reason: UnproposedReason | null }
  | { kind: 'proposed'; requires_restart: boolean };

export interface ThreadSummary {
  thread_id: string;
  title: string;
  channel: string;
  initiator: 'user' | 'system';
  created_at: string;
  last_activity: string;
  message_count: number;
  section: string;
  active_children_count: number;
  total_children_count: number;
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
  liveness?: ThreadLiveness;
}

export interface ThreadsListOptions {
  active?: boolean;
  status?: string;
  source?: string;
  limit?: number;
  parent?: string;
}

function buildListQuery(opts?: ThreadsListOptions): string {
  if (!opts) return '';
  const params = new URLSearchParams();
  if (opts.active !== undefined) params.set('active', String(opts.active));
  if (opts.status !== undefined) params.set('status', opts.status);
  if (opts.source) params.set('source', opts.source);
  if (opts.limit !== undefined) params.set('limit', String(opts.limit));
  if (opts.parent) params.set('parent', opts.parent);
  const s = params.toString();
  return s ? `?${s}` : '';
}

export const threads = {
  list(opts?: ThreadsListOptions): Promise<ThreadSummary[]> {
    return request<ThreadSummary[]>(`/threads/list${buildListQuery(opts)}`);
  },

  count(opts?: Omit<ThreadsListOptions, 'limit'>): Promise<number> {
    return request<{ count: number }>(`/threads/count${buildListQuery(opts)}`)
      .then(r => r.count);
  },

  liveness(threadId: string): Promise<ThreadLiveness> {
    return request<ThreadLiveness>(`/threads/${encodeURIComponent(threadId)}/liveness`);
  },

  users,

  /** @deprecated Use `threads.users`. Kept for apps written before the user
   *  rename (docs/temporary-measures.md, "SDK threadPeople alias"). */
  people,
};
