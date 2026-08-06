import { describe, expect, it } from 'vitest';
import {
  describeWaitSubscription,
  eventWaitProjection,
  formatRemaining,
  secondsRemaining,
} from '../event-waits';
import { resolveVisualStatus } from '../../../components/shared/ThreadStatusIcon';
import type { EventWaitSummary, ThreadEvent } from '../thread-event-types';
import { applyAggregateToMeta, type ThreadAggregate, type ThreadMeta } from '../thread-meta';

/** Only the field the projection touches; everything else on ThreadMeta is
 *  irrelevant here and a full fixture would just rot. */
function meta(waits: EventWaitSummary[] = []): ThreadMeta {
  return { liveEventWaits: waits } as unknown as ThreadMeta;
}

const started = (wait_id: string, reason = 'waiting for the release'): ThreadEvent => ({
  type: 'EventWaitStarted',
  wait_id,
  tool_use_id: 'toolu_1',
  on: [{ event_type: 'ChangeProposed' }],
  reason,
  expires_at: '2026-08-06T12:00:00Z',
  watermark: 10,
});

describe('eventWaitProjection', () => {
  it('appends a started wait, attached', () => {
    const m = meta();
    expect(eventWaitProjection(m, started('w1'))).toBe(true);
    expect(m.liveEventWaits).toHaveLength(1);
    expect(m.liveEventWaits[0]).toMatchObject({ wait_id: 'w1' });
  });

  it('keeps the list a set by wait_id so a replayed event cannot duplicate a wait', () => {
    const m = meta();
    eventWaitProjection(m, started('w1'));
    eventWaitProjection(m, started('w1', 'same wait, replayed'));
    expect(m.liveEventWaits).toHaveLength(1);
    expect(m.liveEventWaits[0].reason).toBe('same wait, replayed');
  });

  it.each([
    ['EventWaitDelivered', { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'e', event_type: 'ChangeProposed', payload: {}, matched_index: 0, was_attached: true }],
    ['EventWaitExpired', { type: 'EventWaitExpired', wait_id: 'w1', was_attached: true }],
    ['EventWaitCanceled', { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' }],
  ] as const)('removes the wait on %s', (_name, event) => {
    const m = meta();
    eventWaitProjection(m, started('w1'));
    eventWaitProjection(m, started('w2'));
    expect(eventWaitProjection(m, event as ThreadEvent)).toBe(true);
    expect(m.liveEventWaits.map((w) => w.wait_id)).toEqual(['w2']);
  });

  /** `await_event`'s own result arrives immediately after registration, and it
   *  must leave the list alone: it is the call's return value, not a state
   *  change. It used to DETACH the wait, which was the client's half of a
   *  distinction ADR 0049 removed. Pinned so that branch does not grow back and
   *  start flickering the indicator on every registration. */
  it('leaves the list alone on the await_event tool result', () => {
    const m = meta();
    eventWaitProjection(m, started('w1'));
    const paired: ThreadEvent = {
      type: 'ToolResult',
      name: 'await_event',
      result: 'Subscribed to ChangeProposed. Nothing is blocking.',
      success: true,
    } as ThreadEvent;
    expect(eventWaitProjection(m, paired)).toBe(false);
    expect(m.liveEventWaits).toHaveLength(1);
  });

  it('ignores a tool result from any other tool', () => {
    const m = meta();
    eventWaitProjection(m, started('w1'));
    const other: ThreadEvent = {
      type: 'ToolResult',
      name: 'run_bash',
      result: 'ok',
      success: true,
    } as ThreadEvent;
    expect(eventWaitProjection(m, other)).toBe(false);
    expect(m.liveEventWaits).toHaveLength(1);
  });

  it('reports no change for an unrelated event', () => {
    const m = meta();
    const unrelated: ThreadEvent = { type: 'ResponseGenerated', text: 'done' } as ThreadEvent;
    expect(eventWaitProjection(m, unrelated)).toBe(false);
  });

  /** Replay must land on exactly the state live SSE built incrementally, or a
   *  reload would show a different set of subscriptions than the open tab. */
  it('replays to the same set the live stream produced', () => {
    const stream: ThreadEvent[] = [
      started('w1'),
      started('w2'),
      { type: 'ToolResult', name: 'await_event', result: 'Subscribed.', success: true } as ThreadEvent,
      { type: 'EventWaitExpired', wait_id: 'w1' } as ThreadEvent,
    ];
    const live = meta();
    const replay = meta();
    for (const e of stream) eventWaitProjection(live, e);
    for (const e of stream) eventWaitProjection(replay, e);
    expect(replay.liveEventWaits).toEqual(live.liveEventWaits);
    expect(live.liveEventWaits.map((w) => w.wait_id)).toEqual(['w2']);
  });
});

/** The status dot reads `meta.liveEventWaitCount`, which comes from the backend
 *  projection, NOT from `meta.liveEventWaits.length`. The two agree whenever
 *  both are populated, and the split is what makes the dot right on a drawer
 *  row whose events were never loaded and after a reload, where the list is
 *  empty by construction. These pin that separation. */
describe('liveEventWaitCount', () => {
  const aggregate = (liveEventWaitCount: number): ThreadAggregate =>
    ({ liveEventWaitCount } as unknown as ThreadAggregate);

  it('is what the Waiting dot reads, with an empty liveEventWaits list', () => {
    const m = { liveEventWaits: [], liveEventWaitCount: 0 } as unknown as ThreadMeta;
    applyAggregateToMeta(m, aggregate(1));
    expect(m.liveEventWaitCount).toBe(1);
    expect(m.liveEventWaits).toHaveLength(0);
    expect(resolveVisualStatus('idle', false, false, m.liveEventWaitCount > 0)).toBe('waiting');
  });

  it('clears the dot when the aggregate reports the last wait resolved', () => {
    const m = { liveEventWaits: [], liveEventWaitCount: 1 } as unknown as ThreadMeta;
    applyAggregateToMeta(m, aggregate(0));
    expect(m.liveEventWaitCount).toBe(0);
    expect(resolveVisualStatus('idle', false, false, m.liveEventWaitCount > 0)).toBe('idle');
  });

  /** The count alone must flag the meta as changed, or `threadMap` never
   *  flushes and the dot does not repaint until some unrelated field moves.
   *  Settling the meta against the same aggregate first is what makes the
   *  count the ONLY difference the second call can see. */
  it('flags a meta change on the count alone, and only when it moves', () => {
    const m = { liveEventWaits: [], liveEventWaitCount: 0 } as unknown as ThreadMeta;
    applyAggregateToMeta(m, aggregate(0));
    expect(applyAggregateToMeta(m, aggregate(0))).toBe(false);
    expect(applyAggregateToMeta(m, aggregate(1))).toBe(true);
    expect(applyAggregateToMeta(m, aggregate(1))).toBe(false);
  });
});

describe('countdown', () => {
  const at = (iso: string) => Date.parse(iso);

  it('floors at zero past the deadline', () => {
    expect(secondsRemaining('2026-08-06T12:00:00Z', at('2026-08-06T12:05:00Z'))).toBe(0);
  });

  it('counts down in whole seconds', () => {
    expect(secondsRemaining('2026-08-06T12:00:00Z', at('2026-08-06T11:59:15Z'))).toBe(45);
  });

  it('answers zero for an unparseable deadline rather than NaN', () => {
    expect(secondsRemaining('not a date', Date.now())).toBe(0);
  });

  it.each([
    [0, 'due now'],
    [18, '18s'],
    [252, '4m 12s'],
    [7500, '2h 5m'],
  ])('formats %i seconds as %s', (secs, text) => {
    expect(formatRemaining(secs)).toBe(text);
  });
});

describe('describeWaitSubscription', () => {
  it('joins entries with "or", matching the per-entry OR the matcher runs', () => {
    expect(
      describeWaitSubscription([{ event_type: 'ChangeProposed' }, { event_type: 'ResponseGenerated' }]),
    ).toBe('ChangeProposed or ResponseGenerated');
  });

  /** The raw operator JSON is developer-facing; the person reading this row is
   *  whoever is waiting. */
  it('summarises a condition instead of dumping the operator object', () => {
    const text = describeWaitSubscription([
      { event_type: 'ChangeProposed', condition: { file_count: { $gt: 0 } } },
    ]);
    expect(text).toBe('ChangeProposed (filtered)');
    expect(text).not.toContain('$gt');
  });
});
