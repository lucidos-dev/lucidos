import { describe, expect, it } from 'vitest';
import {
  describeWaitSubscription,
  displayedWaitDeadline,
  eventWaitProjection,
  formatRemaining,
  groupSubscriptions,
  plainEventName,
  secondsRemaining,
} from '../event-waits';
import { resolveVisualStatus } from '../../../components/shared/threadVisualStatus';
import { awaitedSubject, eventWaitStoppedSummary, waitingFor } from '../thread-event-types';
import type { EventWaitSummary, ThreadEvent } from '../thread-event-types';
import { applyAggregateToMeta, type ThreadAggregate, type ThreadMeta } from '../thread-meta';

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

it('chooses the live deadline from typed timing and preserves manual waits', () => {
  const expires_at = '2026-08-06T13:00:00Z';
  const manual = { wait_id: 'manual', on: [], reason: 'a build slot', expires_at };
  const deadline = '2026-08-06T12:55:00Z';
  expect(displayedWaitDeadline(manual)).toBe(expires_at);
  expect(displayedWaitDeadline({ ...manual, background_timing: { state: 'queued' } })).toBe('');
  expect(displayedWaitDeadline({ ...manual, background_timing: { state: 'running', deadline } })).toBe(deadline);
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
    ['EventWaitDelivered', { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'e', event_type: 'ChangeProposed', payload: {}, matched_index: 0 }],
    ['EventWaitExpired', { type: 'EventWaitExpired', wait_id: 'w1' }],
    ['EventWaitCanceled', { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' }],
  ] as const)('removes the wait on %s', (_name, event) => {
    const m = meta();
    eventWaitProjection(m, started('w1'));
    eventWaitProjection(m, started('w2'));
    expect(eventWaitProjection(m, event as ThreadEvent)).toBe(true);
    expect(m.liveEventWaits.map((w) => w.wait_id)).toEqual(['w2']);
  });

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

it('reconciles a missed timing update on the same wait ID', () => {
  const m = meta();
  eventWaitProjection(m, started('w1', 'a build slot'));
  const previous = m.liveEventWaits[0];
  const running = {
    ...previous,
    reason: 'the build to finish',
    background_timing: { state: 'running', deadline: '2026-08-06T11:55:00Z' } as const,
  };
  const summary = { liveEventWaits: [running] } as ThreadAggregate;
  expect(applyAggregateToMeta(m, summary)).toBe(true);
  expect(m.liveEventWaits).toEqual([running]);
  expect(applyAggregateToMeta(m, summary)).toBe(false);
});

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
    ).toBe('change proposed or response generated');
  });

  it('summarises a condition instead of dumping the operator object', () => {
    const text = describeWaitSubscription([
      { event_type: 'ChangeProposed', condition: { file_count: { $gt: 0 } } },
    ]);
    expect(text).toBe('change proposed (with a condition)');
    expect(text).not.toContain('$gt');
  });

  it('names a repeated event type once, with its condition count', () => {
    const on = [1, 2, 3].map((n) => ({ event_type: 'CodingAgentIdled', condition: { n } }));
    expect(describeWaitSubscription([...on, { event_type: 'ChangeProposed' }])).toBe(
      'coding agent stopped working (3 conditions) or change proposed',
    );
  });
});

describe('plainEventName', () => {
  it.each([
    ['BackgroundBashCompleted', 'background job finished'],
    ['CodingAgentIdled', 'coding agent stopped working'],
    ['ChangeProposed', 'change proposed'],
    ['E2ELockReleased', 'test lock released'],
  ])('uses the everyday name for %s', (type, words) => {
    expect(plainEventName(type)).toBe(words);
  });

  it.each([
    ['GithubWorkflowRunStateChanged', 'github workflow run state changed'],
    ['SSEConnectionDropped', 'SSE connection dropped'],
    ['ReportV2Published', 'report V2 published'],
  ])('splits any other type into words: %s', (type, words) => {
    expect(plainEventName(type)).toBe(words);
  });

  it.each(['', 'lowercase_name'])('leaves an odd input readable: %s', (type) => {
    expect(plainEventName(type)).toBe(type === '' ? '' : 'lowercase name');
  });
});

describe('groupSubscriptions', () => {
  it('keeps first-seen order and collects each type\'s conditions', () => {
    expect(groupSubscriptions([
      { event_type: 'A', condition: { x: 1 } },
      { event_type: 'B' },
      { event_type: 'A', condition: { x: 2 } },
    ])).toEqual([
      { event_type: 'A', conditions: [{ x: 1 }, { x: 2 }] },
      { event_type: 'B', conditions: [] },
    ]);
  });

  it('drops the conditions of a type that is also watched unfiltered', () => {
    expect(groupSubscriptions([
      { event_type: 'A', condition: { x: 1 } },
      { event_type: 'A' },
    ])).toEqual([{ event_type: 'A', conditions: [] }]);
  });
});

describe('awaitedSubject', () => {
  it.each([
    ['waiting for the e2e lock', 'the e2e lock'],
    ['waiting on E2ELockReleased', 'E2ELockReleased'],
    ['waiting until the nightly reports', 'the nightly reports'],
    ['wait for the release build', 'the release build'],
    ['Waiting for tonight\'s e2e run', 'tonight\'s e2e run'],
    ['WAITING FOR the apply', 'the apply'],
    ['waiting  for  the lock', 'the lock'],
  ])('drops the duplicated verb in %s', (reason, subject) => {
    expect(awaitedSubject(reason)).toBe(subject);
  });

  it.each([
    'the release build to finish',
    'tonight\'s E2E suite to pass',
    'the lock, which another run is waiting for too',
    'waiting',
    'waited for the lock',
    'waiting formally',
  ])('leaves %s exactly as written', (reason) => {
    expect(awaitedSubject(reason)).toBe(reason);
  });

  it.each(['waiting for', 'waiting for ', '  waiting until   '])(
    'returns %s whole rather than emptying it',
    (reason) => {
      expect(awaitedSubject(reason)).toBe(reason);
    },
  );
});

describe('waitingFor', () => {
  it.each([
    ['the release build to finish', 'Waiting for the release build to finish'],
    ['waiting for the e2e lock', 'Waiting for the e2e lock'],
    ['The nightly import', 'Waiting for the nightly import'],
    ['E2E suite to pass', 'Waiting for E2E suite to pass'],
    ['something to change', 'Waiting for something to change'],
    ['morning report to land', 'Waiting for morning report to land'],
    ['Pending CI checks', 'Waiting for Pending CI checks'],
    ['watching the deploy', 'Waiting: watching the deploy'],
    ['Watching background work started in this thread', 'Waiting: Watching background work started in this thread'],
  ])('frames %s', (reason, label) => {
    expect(waitingFor('Waiting', reason)).toBe(label);
  });
});

describe('eventWaitStoppedSummary', () => {
  it('does not say "waiting" twice', () => {
    expect(eventWaitStoppedSummary('waiting for the e2e lock')).toBe(
      'Stopped waiting for the e2e lock',
    );
  });

  it('keeps a reason that never duplicated the verb', () => {
    expect(eventWaitStoppedSummary('the release build to finish')).toBe(
      'Stopped waiting for the release build to finish',
    );
  });

  it.each([undefined, ''])('falls back for a legacy row carrying %s', (reason) => {
    expect(eventWaitStoppedSummary(reason)).toBe('Stopped waiting for an event');
  });
});
