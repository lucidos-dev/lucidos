import type { CacheKeepAliveStatus, CeilingCrossing, HandoffOpening } from '../../generated/thread-event-wire';
import type { CodingAgent } from '../types';
import { API, json, mutatingFetch, throwIfNotOk } from './_core';

// --- Successor sessions ---
//
// GET /threads/:id/context reports a coding-agent thread's size and what the
// Successor card makes of it. POST /threads/:id/successor-offer/continue and
// /decline are the card's Continue and Stay here. POST
// /threads/:id/successor/withdraw is the ceiling card's Return. GET
// /threads/:id/switch-estimate prices a model or effort switch.

/** The Successor card's reading: whether a handoff pays on either cache state.
 *  The engine owns the estimate. The browser only picks the state by the clock. */
export interface SuccessorCardVerdict {
  /** The card shows while the prompt cache is warm. */
  warm: boolean;
  /** The card shows once the cache is cold. */
  cold: boolean;
  /** When the cache goes cold, an ISO time. Absent when it already is. */
  cold_at?: string;
  /** How many times a fresh session's context the thread holds. */
  fresh_ratio: number;
}

/** A thread's context size, as the engine measures it. The engine owns the
 *  thresholds and knows which threads can hand off, so callers act on `offer`
 *  and `card` and never compare `input_tokens` themselves. */
export interface ThreadContext {
  /** Input tokens of the thread's latest main-session model call. `null`
   *  when it has made none. */
  input_tokens: number | null;
  /** The thread may start a successor at all, at any size. */
  can_hand_off: boolean;
  /** Large enough to offer a new thread after an Apply. */
  offer: boolean;
  /** The thread has an applied change, so a new request can go to a
   *  successor after it. */
  applied: boolean;
  /** The Successor card's verdict. Absent when the engine can make no reading. */
  card?: SuccessorCardVerdict;
  /** A handoff at the *context ceiling* waiting for the next input. */
  ceiling_handoff?: CeilingHandoff;
}

/** A ceiling handoff waiting for the thread's next input (ADR 0629). */
export interface CeilingHandoff {
  opening: HandoffOpening;
  crossing: CeilingCrossing;
  /** The ceiling Return to the old session sets, in tokens. `null` when the
   *  context window keeps every ceiling under the session's size. */
  raises_to: number | null;
}

export async function fetchThreadContext(threadId: string): Promise<ThreadContext> {
  return json<ThreadContext>(`${API}/threads/${encodeURIComponent(threadId)}/context`);
}

/** The backend, model and effort a successor session runs on. A field left
 *  out keeps the current session's. */
export interface SuccessorImplementer {
  coding_agent?: CodingAgent;
  coding_agent_model?: string;
  reasoning_effort?: string | null;
}

/** What Continue returns. The engine picks the path by cache warmth: `brief`
 *  started the *successor session* at once, `narrative` asked the agent to
 *  write the handoff first. Either way the session runs in this same thread. */
export type ContinueOutcome = { path: 'narrative' } | { path: 'brief'; starts: 'now' };

/** Continue on the Successor card. `message` is the user's first request for
 *  the successor session, if they typed one. */
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

/** Stay here on the Successor card: hide it until the thread grows. */
export async function declineSuccessorOffer(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/successor-offer/decline`, {
    method: 'POST',
  });
  await throwIfNotOk(res);
}

/** Return to the old session: withdraw the pending ceiling handoff. The engine
 *  raises the thread's ceiling past the session's size first. */
export async function withdrawCeilingHandoff(threadId: string): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/successor/withdraw`, {
    method: 'POST',
  });
  await throwIfNotOk(res);
}

/** Whether a switch rewrites the thread's prompt cache. `unknown`: the cache
 *  may still be warm, since no evidence gives its lifetime. */
export type CacheRebuild = 'yes' | 'no' | 'unknown';

/** What switching a coding-agent thread's model or effort would cost, as the
 *  engine prices it. The browser prices nothing itself. */
export interface SwitchEstimate {
  context_tokens: number | null;
  /** `yes` on a warm cache. A cold cache and a no-op never rebuild. */
  rebuilds_cache: CacheRebuild;
  /** One time, in USD, should it rewrite. `null` when nothing is rewritten or
   *  a model has no price. */
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

/** How long a thread's prompt cache stays warm, as the engine reads it. The
 *  keep-alive status carries its own prices. */
export type CacheWarmth =
  /** A cold cache, or nothing cached. */
  | { warmth: 'cold'; remaining_secs: null; keep_alive: null; ttl: null }
  /** No evidence gives the cache's lifetime, so it may be warm or cold. */
  | { warmth: 'unknown'; remaining_secs: null; keep_alive: CacheKeepAliveStatus; ttl: null }
  | { warmth: 'warm'; remaining_secs: number; keep_alive: CacheKeepAliveStatus; ttl: WarmthTtl | null };

/** The lifetime a warm cache was measured on, and what says so: the
 *  response's split of the write, or the lifetime the request asked for. */
export interface WarmthTtl {
  lifetime: '5m' | '30m' | '1h' | '12h';
  evidence: 'reported' | 'requested';
}

export async function fetchCacheWarmth(threadId: string): Promise<CacheWarmth> {
  return json<CacheWarmth>(`${API}/threads/${encodeURIComponent(threadId)}/cache-warmth`);
}

/** Turn the cache keep-alive off for one thread until it runs again, or back
 *  on (ADR 0554). */
export async function switchCacheKeepAlive(threadId: string, state: 'off' | 'on'): Promise<void> {
  const res = await mutatingFetch(`${API}/threads/${encodeURIComponent(threadId)}/cache-keep-alive`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ state }),
  });
  await throwIfNotOk(res);
}
