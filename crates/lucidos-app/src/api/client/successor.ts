import type { CacheKeepAliveStatus, CeilingCrossing, HandoffOpening } from '../../generated/thread-event-wire';
import type { CodingAgent } from '../types';
import { API, json, mutatingFetch, throwIfNotOk } from './_core';


export interface SuccessorCardVerdict {
  warm: boolean;
  cold: boolean;
  cold_at?: string;
  fresh_ratio: number;
}

export interface ThreadContext {
  input_tokens: number | null;
  can_hand_off: boolean;
  offer: boolean;
  applied: boolean;
  card?: SuccessorCardVerdict;
  ceiling_handoff?: CeilingHandoff;
}

export interface CeilingHandoff {
  opening: HandoffOpening;
  crossing: CeilingCrossing;
  raises_to: number | null;
}

export async function fetchThreadContext(threadId: string): Promise<ThreadContext> {
  return json<ThreadContext>(`${API}/threads/${encodeURIComponent(threadId)}/context`);
}

export interface SuccessorImplementer {
  coding_agent?: CodingAgent;
  coding_agent_model?: string;
  reasoning_effort?: string | null;
}

export type ContinueOutcome = { path: 'narrative' } | { path: 'brief'; starts: 'now' };

export async function continueInSuccessor(
  threadId: string,
  message: string | undefined,
  implementer?: SuccessorImplementer,
): Promise<ContinueOutcome> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/successor-offer/continue`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ message, ...implementer }),
  });
  await throwIfNotOk(res);
  return res.json();
}

export async function declineSuccessorOffer(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/successor-offer/decline`, {
    method: 'POST',
  });
  await throwIfNotOk(res);
}

export async function withdrawCeilingHandoff(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/successor/withdraw`, {
    method: 'POST',
  });
  await throwIfNotOk(res);
}

export type CacheRebuild = 'yes' | 'no' | 'unknown';

export interface SwitchEstimate {
  context_tokens: number | null;
  rebuilds_cache: CacheRebuild;
  extra_cost_usd: number | null;
}

export async function fetchSwitchEstimate(
  threadId: string,
  model: string,
  effort: string | null,
): Promise<SwitchEstimate> {
  const params = new URLSearchParams({ model });
  if (effort) params.set('effort', effort);
  return json<SwitchEstimate>(`${API}/threads/${encodeURIComponent(threadId)}/switch-estimate?${params}`);
}

export type CacheWarmth =
  | { warmth: 'cold'; remaining_secs: null; keep_alive: null; ttl: null }
  | { warmth: 'unknown'; remaining_secs: null; keep_alive: CacheKeepAliveStatus; ttl: null }
  | { warmth: 'warm'; remaining_secs: number; keep_alive: CacheKeepAliveStatus; ttl: WarmthTtl | null };

export interface WarmthTtl {
  lifetime: '5m' | '30m' | '1h' | '12h';
  evidence: 'reported' | 'requested';
}

export async function fetchCacheWarmth(threadId: string): Promise<CacheWarmth> {
  return json<CacheWarmth>(`${API}/threads/${encodeURIComponent(threadId)}/cache-warmth`);
}

export async function switchCacheKeepAlive(threadId: string, state: 'off' | 'on'): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/cache-keep-alive`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ state }),
  });
  await throwIfNotOk(res);
}
