/** How a turn parked on an *event wait* reads in the transcript (ADR 0047).
 *
 *  `await_event` is terminal: registering the wait ends the turn, and the
 *  engine deliberately emits no terminator, because the dangling
 *  `ToolCalled{await_event}` IS the rendezvous slot the delivered event lands
 *  in. Everything below follows from that one fact: the projection has to read
 *  a turn with no terminator and a permanently pending tool call as *finished*
 *  rather than as *still working*, and it has to say so once, not twice.
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter } from './thread-flows-helpers';
import { exchangeResponseEvents, exchangeStatus, type ThreadEvent } from '../thread-events';
import { getEventToggleState } from '../event-rendering';
import type { ResponseEvent } from '../types';

beforeEach(resetSeqCounter);

const REASON = 'the release build to finish';

const park: ThreadEvent[] = [
  { type: 'MessageReceived', text: 'tell me when the build lands' },
  { type: 'ToolCalled', name: 'await_event', args: { reason: REASON }, description: `Waiting: ${REASON}` },
  {
    type: 'EventWaitStarted',
    wait_id: 'w1',
    tool_use_id: 'toolu_1',
    on: [{ event_type: 'ChangeProposed' }],
    reason: REASON,
    expires_at: '2026-08-06T12:00:00Z',
    watermark: 10,
  },
];

const waits = (events: ResponseEvent[]) => events.filter((e) => e.type === 'event_wait');
const steps = (events: ResponseEvent[]) => events.filter((e) => e.type === 'step');

describe('a turn parked on an event wait', () => {
  /** The park used to render twice: the generic `Waiting: <reason>` tool step
   *  AND the event-wait row right under it, both naming the same reason. The
   *  row is the richer of the two (it carries the subscription, the resolution
   *  state and the jump to the matched event), so it takes the tool step's
   *  place rather than queueing behind it. */
  it('renders as ONE row, the event-wait line replacing the await_event tool step', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(waits(events)).toHaveLength(1);
    expect(steps(events)).toHaveLength(0);
    expect(waits(events)[0]).toMatchObject({ wait_id: 'w1', state: 'waiting', subscription: 'ChangeProposed' });
  });

  /** Steps are collapsed by default, and the event-wait row is gated on the
   *  same flag as every other step row. Since the row replaced the only
   *  `'step'` this turn had, the toggle has to count it or the turn renders as
   *  an empty response with no way to open it. */
  it('still offers the Show steps toggle that reveals the row', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const { showStepsToggle } = getEventToggleState(exchangeResponseEvents(getExchanges(map, id)[0]));
    expect(showStepsToggle).toBe(true);
  });

  /** Setting the wait up is an action that COMPLETED. The thread sleeping
   *  afterwards is the subscription indicator's business, not this row's, so
   *  the row must not shimmer as in-progress for however many hours the wait
   *  runs. */
  it('reads as a finished step, not an in-progress one', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(events.some((e) => e.type === 'step' && e.outcome === 'pending')).toBe(false);
  });

  /** No terminator by design, so the generic "steps but nothing ended it"
   *  fallthrough called this turn `streaming` and the panel read "Working" for
   *  the entire park. The turn is over: it produced its work and parked. */
  it('reports the turn as done rather than working', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const exchange = getExchanges(map, id)[0];
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(getLabel(exchange)).toBe('Done');
  });

  /** An attached delivery resumes THIS exchange: the wake's steps land under
   *  the same row, which flips to `woke`, and the turn is live again until its
   *  own terminator. */
  it('goes back to working when an attached delivery wakes it', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'evt-9', event_type: 'ChangeProposed', payload: {}, matched_index: 0, was_attached: true },
      { type: 'ToolResult', name: 'await_event', result: 'ChangeProposed fired' },
      { type: 'ToolCalled', name: 'send_notification', args: { title: 'Build landed' } },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    const events = exchangeResponseEvents(exchange);
    expect(waits(events)).toHaveLength(1);
    // No `matched_event_id` assertion: `insertEvents` reads a top-level
    // `event_id` as the row's OWN id and strips it, so the delivery's
    // same-named payload field cannot survive this helper.
    expect(waits(events)[0]).toMatchObject({ state: 'woke', matched_event_type: 'ChangeProposed' });
    // The wake's own tool call is the only step, and it is the one still running.
    expect(steps(events)).toHaveLength(1);
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  /** The `await_event` result belongs to a step that no longer exists. Left to
   *  the generic "resolve the last pending step" walker it would tick off
   *  whatever unrelated call happened to be in flight. */
  it('never resolves an unrelated step with the await_event result', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'evt-9', event_type: 'ChangeProposed', payload: {}, matched_index: 0, was_attached: true },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      { type: 'ToolResult', name: 'await_event', result: 'ChangeProposed fired' },
    ] as ThreadEvent[]);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(steps(events)).toHaveLength(1);
    expect(steps(events)[0]).toMatchObject({ outcome: 'pending' });
  });

  /** A rejected subscription never produces an `EventWaitStarted`, so nothing
   *  replaces the tool step and the failed call must stay visible with its
   *  error, like any other tool call. */
  it('keeps an ordinary tool step when the wait was never registered', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'watch for nonsense' },
      { type: 'ToolCalled', name: 'await_event', args: { reason: REASON }, description: `Waiting: ${REASON}` },
      { type: 'ToolResult', name: 'await_event', result: 'Error: unknown event type' },
      { type: 'ResponseGenerated', text: 'That event type does not exist.' },
    ] as ThreadEvent[]);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(waits(events)).toHaveLength(0);
    expect(steps(events)).toHaveLength(1);
    expect(steps(events)[0]).toMatchObject({ outcome: 'success', result: 'Error: unknown event type' });
  });

  /** A deadline wakes the model exactly like a delivery does (it gets a
   *  "never seen" tool result and carries on), so the turn is live again. */
  it('goes back to working when the deadline passes', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitExpired', wait_id: 'w1', was_attached: true },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(waits(exchangeResponseEvents(exchange))[0]).toMatchObject({ state: 'timed_out' });
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  /** A cancel is the one resolution with no wake behind it: the engine closes
   *  the dangling call so the next turn is sendable, and the thread settles to
   *  idle. Settled plus no terminator is exactly the shape the stale detector
   *  calls a crash, so this turn has to keep reading as the finished thing it
   *  is. The row's own muted note says the wait was stopped. */
  it('stays done when the user stops the wait, and never reads as aborted', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', was_attached: true },
      { type: 'ToolResult', name: 'await_event', result: 'The wait was canceled.' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(waits(exchangeResponseEvents(exchange))[0]).toMatchObject({ state: 'canceled' });
    expect(exchangeStatus(exchange, '', true, false, false, /* threadIdle */ true)).toBe('done');
  });
});
