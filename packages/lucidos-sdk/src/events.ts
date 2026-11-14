import { request, requestVoid } from './_fetch';
import { assertPlainObject, assertString } from './_validate';

/**
 * Filters for `lucidos.events.query`. Mirrors the engine's `EventQueryFilters`
 * (`crates/lucidos-engine/src/core/store/mod.rs`), which is the source of truth.
 *
 * `before_event_id` and `after_event_id` are **mutually exclusive**: there is no
 * coherent meaning for "strictly older than X AND strictly newer than Y" in a
 * paging API, so the engine's `validate_cursor_pair` rejects the pair with a 400
 * before running the query. A cursor uuid that resolves to no event is a 404,
 * never a silently unfiltered page.
 */
export interface EventQuery {
  event_type?: string;
  since?: string;
  until?: string;
  limit?: number;
  /**
   * Walk backward from this event, exclusive, under `(created, id)`
   * lexicographic order. This is how you page through history.
   */
  before_event_id?: string;
  /** Tail-follow forward from this event, exclusive. */
  after_event_id?: string;
  /**
   * Restrict to one thread. Absent is every thread, which is what every caller
   * predating this field passes, so the filter can only narrow.
   */
  thread_id?: string;
  /**
   * Resolve ONE event by primary key, rather than positioning a window the way
   * the two cursors above do. Takes a bare uuid, or the `evt-<32 hex>` address
   * the agent sees on a tool result. A malformed value is a 400, never a
   * silently unfiltered page.
   */
  event_id?: string;
}

/**
 * Build the querystring `query()` sends. Split out as a pure function so every
 * field of `EventQuery` is testable: a field declared on the interface but not
 * forwarded here fails silently at runtime (the engine just ignores what it
 * never received), which is exactly how `before_event_id` shipped as a no-op
 * and left callers paging the same first page forever.
 */
export function buildEventQueryString(params: EventQuery): string {
  const qs = new URLSearchParams();
  if (params.event_type) qs.set('event_type', params.event_type);
  if (params.since) qs.set('since', params.since);
  if (params.until) qs.set('until', params.until);
  if (params.limit != null) qs.set('limit', String(params.limit));
  if (params.before_event_id) qs.set('before_event_id', params.before_event_id);
  if (params.after_event_id) qs.set('after_event_id', params.after_event_id);
  if (params.thread_id) qs.set('thread_id', params.thread_id);
  if (params.event_id) qs.set('event_id', params.event_id);
  return qs.toString();
}

/**
 * One row from the workspace's event store, as returned by
 * `lucidos.events.query`. Mirrors the engine's `EventRow`
 * (`crates/lucidos-engine/src/core/events.rs`), which is the source of truth.
 *
 * The query reads the WHOLE `events` table: workspace-emitted domain events
 * (`HabitCompleted`) and engine thread/system events (`ChildThreadCompleted`,
 * `ResponseGenerated`, `ChangeApplied`) live in one table and come back from
 * one call. See `system-knowhow/thread-events.md` § "One table, two enums".
 */
export interface LucidosEvent {
  id: string;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  /**
   * The thread this event belongs to. Present on engine thread events only
   * (for `ChildThreadCompleted` it is the PARENT thread); absent, not null,
   * on workspace-emitted domain events and other system events.
   */
  thread_id?: string;
  /** Monotonic insertion order across the workspace. Always present here. */
  sequence: number;
}

export interface EmitOptions {
  /** Skip persistence — broadcast on SSE only. */
  transient?: boolean;
}

export const events = {
  query(params: EventQuery = {}): Promise<LucidosEvent[]> {
    const q = buildEventQueryString(params);
    return request(`/events/query${q ? `?${q}` : ''}`);
  },

  /**
   * Emit a domain event. By default the event is persisted to the event store
   * and broadcast on SSE. Pass `{ transient: true }` for ephemeral coordination
   * signals (heartbeats, presenter↔remote state) that should reach SSE
   * consumers but not be written to the event store.
   */
  emit(type: string, payload: Record<string, unknown>, options: EmitOptions = {}): Promise<void> {
    assertString('type', type);
    assertPlainObject('payload', payload);
    const body: Record<string, unknown> = { event_type: type, payload };
    if (options.transient) body.transient = true;
    return requestVoid('/events/emit', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
  },
};
