import { request, requestVoid } from './_fetch';
import { assertPlainObject, assertString } from './_validate';

export interface EventQuery {
  event_type?: string;
  since?: string;
  until?: string;
  limit?: number;
  before_event_id?: string;
  after_event_id?: string;
  thread_id?: string;
  event_id?: string;
}

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

export interface LucidosEvent {
  id: string;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  thread_id?: string;
  sequence: number;
}

export interface EmitOptions {
  transient?: boolean;
}

export const events = {
  query(params: EventQuery = {}): Promise<LucidosEvent[]> {
    const q = buildEventQueryString(params);
    return request(`/events/query${q ? `?${q}` : ''}`);
  },

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
