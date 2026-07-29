import { request, requestVoid } from './_fetch';
import { assertPlainObject, assertString } from './_validate';

export interface EventQuery {
  event_type?: string;
  since?: string;
  until?: string;
  limit?: number;
}

export interface LucidosEvent {
  id: string;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  aggregate?: string;
  aggregate_id?: string;
}

export interface EmitOptions {
  /** Skip persistence — broadcast on SSE only. */
  transient?: boolean;
}

export const events = {
  query(params: EventQuery = {}): Promise<LucidosEvent[]> {
    const qs = new URLSearchParams();
    if (params.event_type) qs.set('event_type', params.event_type);
    if (params.since) qs.set('since', params.since);
    if (params.until) qs.set('until', params.until);
    if (params.limit != null) qs.set('limit', String(params.limit));
    const q = qs.toString();
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
