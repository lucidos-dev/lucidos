import { describe, it, expect } from 'vitest';
import { buildEventQueryString } from './events';
import type { EventQuery } from './events';

// The engine's `EventsQueryParams`
// (`crates/lucidos-engine/src/api/history.rs`) is the wire contract. A param
// the SDK declares but never puts on the querystring is dropped in silence:
// the engine answers 200 with the unfiltered page, so a caller paging with
// `before_event_id` gets the identical first page forever and looks correct
// while doing it. These tests exist to make that failure loud.
describe('buildEventQueryString', () => {
  it('emits nothing for an empty query', () => {
    expect(buildEventQueryString({})).toBe('');
  });

  it('forwards every field the engine accepts', () => {
    const params: Required<EventQuery> = {
      event_type: 'ContextCaptured',
      since: '2026-08-01T00:00:00Z',
      until: '2026-08-02T00:00:00Z',
      limit: 500,
      before_event_id: '11111111-1111-4111-8111-111111111111',
      after_event_id: '22222222-2222-4222-8222-222222222222',
      thread_id: '33333333-3333-4333-8333-333333333333',
      event_id: '44444444-4444-4444-8444-444444444444',
    };
    const got = new URLSearchParams(buildEventQueryString(params));
    for (const [key, value] of Object.entries(params)) {
      expect(got.get(key), `${key} was dropped`).toBe(String(value));
    }
  });

  // Asserted through URLSearchParams rather than against a literal string:
  // param order is irrelevant to the engine, so pinning it would fail a pure
  // reorder of the `set` calls for no behavioural reason.
  it('carries the paging cursor and omits the params left unset', () => {
    const got = new URLSearchParams(
      buildEventQueryString({
        event_type: 'ContextCaptured',
        before_event_id: '11111111-1111-4111-8111-111111111111',
        limit: 200,
      }),
    );
    expect(got.get('event_type')).toBe('ContextCaptured');
    expect(got.get('before_event_id')).toBe('11111111-1111-4111-8111-111111111111');
    expect(got.get('limit')).toBe('200');
    expect(got.has('after_event_id')).toBe(false);
    expect(got.has('thread_id')).toBe(false);
    expect(got.has('event_id')).toBe(false);
    expect(got.has('since')).toBe(false);
    expect(got.has('until')).toBe(false);
  });

  it('keeps limit=0 rather than treating it as absent', () => {
    expect(buildEventQueryString({ limit: 0 })).toBe('limit=0');
  });
});
