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
import { getCollapsedVisibleEvents, getEventToggleState } from '../event-rendering';
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

  /** The row renders on its own, so a turn whose ONLY action was the park has
   *  no step mechanics left to reveal and must not grow a button that reveals
   *  nothing. The row used to be gated on "Show steps" and was counted here so
   *  the toggle existed to open it; both halves went away together. */
  it('offers no Show steps toggle when the park was the only action', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const { showStepsToggle } = getEventToggleState(exchangeResponseEvents(getExchanges(map, id)[0]));
    expect(showStepsToggle).toBe(false);
  });

  /** **The row survives the default view, which is the whole bug.**
   *
   *  `await_event`'s result tells the model to finish its turn and end the
   *  response normally, so an arming turn reliably writes prose AFTER the park.
   *  Two prose chunks turn the More/Less collapse on, and it defaults to
   *  collapsed, so the row sat before the last text block and was filtered out
   *  of the rendered set. The event was in the stream, the class was in the
   *  bundle, and no element ever painted.
   *
   *  Shape taken from real thread ce806a06: the agent reports, arms the wait,
   *  then says what it is now watching. */
  it('survives the collapsed view of a turn that talks after arming', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'apply it' },
      { type: 'ToolCalled', name: 'changes', args: { action: 'apply' }, description: 'Applying change...' },
      { type: 'ToolResult', name: 'changes', result: '{"status":"conflict"}' },
      { type: 'TextStreamed', text: 'The merge hit a conflict against main.' },
      { type: 'ToolCalled', name: 'await_event', args: { reason: REASON }, description: `Waiting: ${REASON}` },
      ...park.slice(2),
      { type: 'ToolResult', name: 'await_event', result: 'Subscribed to ChangeApplied.' },
      { type: 'TextStreamed', text: "I'm subscribed to the apply, so I'll come back here." },
      { type: 'ResponseGenerated', text: "I'm subscribed to the apply, so I'll come back here." },
    ] as ThreadEvent[]);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    // The collapse is on by default for this turn: it has steps and two prose
    // chunks, and `detailsExpanded` starts false.
    expect(getEventToggleState(events).showMoreToggle).toBe(true);
    expect(waits(getCollapsedVisibleEvents(events).visibleEvents)).toMatchObject([
      { wait_id: 'w1', state: 'waiting', reason: REASON },
    ]);
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

  /** The park's own turn stays done when the user stops the wait, and never
   *  reads as aborted. A cancel is the one resolution with no wake behind it:
   *  the engine closes the dangling call so the next turn is sendable, and the
   *  thread settles to idle. Settled plus no terminator is exactly the shape
   *  the stale detector calls a crash. */
  it('leaves the parked turn reading done when the user stops the wait', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', was_attached: true },
      { type: 'ToolResult', name: 'await_event', result: 'The wait was canceled.' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(exchangeStatus(exchange, '', false, false, false, /* threadIdle */ true)).toBe('done');
  });

  /** **A user stop is the user's own turn, and never a rewrite of the arming
   *  row.**
   *
   *  The stop used to flip the arming row in place: "Set up an event wait:
   *  Waiting for tonight's E2E suite to pass" became a struck "Stopped waiting:
   *  Waiting for to…", sitting directly above the agent's own "I'm now watching
   *  for E2ETestsPassed" prose. Two things were wrong at once. The transcript
   *  lost when the watch STARTED (that turn really did arm a subscription), and
   *  the thing the user actually did last, hours later, was reported by editing
   *  something near the top of the thread.
   *
   *  It is a turn of its own for the same reason a Stop or a Restart is: a
   *  person did something to this thread at a moment, and the moment is the
   *  point. */
  it('opens the stop as its own turn rather than rewriting the arming row', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      {
        type: 'EventWaitCanceled',
        wait_id: 'w1',
        cause: 'user_stop',
        on: [{ event_type: 'ChangeProposed' }],
        reason: REASON,
        actor: { kind: 'device', device_id: 'dev-1', label: 'My iPhone' },
      },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    // The arming turn is untouched: it still says what it did, when it did it.
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([
      { state: 'waiting', reason: REASON },
    ]);
    // The stop is the last turn, anchored on the stop itself, and draws no step
    // row of its own: its header line IS the record.
    expect(exchanges[1].userEvent.type).toBe('EventWaitCanceled');
    expect(exchangeResponseEvents(exchanges[1])).toHaveLength(0);
  });

  /** Same shape when the user stops within seconds, before the arming turn has
   *  scrolled anywhere: the arming row and the stop are still two actions, and
   *  the one the user took is still theirs. This is the case the in-place flip
   *  handled and got wrong, so it is pinned separately. */
  it('opens its own turn even when the stop lands in the arming turn', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', reason: REASON },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([{ state: 'waiting' }]);
  });

  /** The stop turn states its own outcome and nothing continues out of it, so
   *  it is terminal whatever the thread is doing. A stop while an unrelated turn
   *  is still running must not spin "Requesting", and once the thread settles it
   *  must not fall through to the stale detector's "Aborted". */
  it.each([
    ['idle', true],
    ['running', false],
  ] as const)('reads the stop turn as done on a %s thread', (_label, threadIdle) => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', reason: REASON },
    ] as ThreadEvent[]);

    const stop = getExchanges(map, id)[1];
    expect(exchangeStatus(stop, '', true, false, false, threadIdle)).toBe('done');
    expect(getLabel(stop)).toBe('Done');
  });

  /** **The stop turn owns nothing that follows it.**
   *
   *  A subscription does not hold its thread's turn and the Stop waiting button
   *  has no idle guard, so the user can press it while an unrelated turn is
   *  mid-flight, and that turn carries on afterwards. Everything routed
   *  CHRONOLOGICALLY rather than by request id (every coding-agent event, a chat
   *  `TodoListWritten`, a background-bash pair) must keep landing in the turn
   *  that produced it. Folded into the stop instead it would draw nothing at
   *  all, since the stop panel renders no response body. */
  it('keeps a running turn owning its own work when a stop lands mid-flight', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      { type: 'MessageReceived', text: 'meanwhile, tidy the notes' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      // The user opens the clock indicator mid-turn and stops the old watch.
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', reason: REASON },
      // Work from the turn that never stopped. Chronologically routed, so
      // `current` is what decides where it lands.
      { type: 'TodoListWritten', items: [{ content: 'tidy', status: 'in_progress' }] },
      { type: 'BackgroundBashStarted', bash_id: 'b1', command: 'ls' },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    // The stop is its own turn and acquired nothing.
    expect(exchanges[2].userEvent.type).toBe('EventWaitCanceled');
    expect(exchanges[2].steps).toHaveLength(0);
    // The running turn kept every one of its own events.
    expect(exchanges[1].steps.map((s) => s.event.type)).toEqual([
      'ToolCalled',
      'TodoListWritten',
      'BackgroundBashStarted',
    ]);
  });

  /** **A stand-down is the agent's action inside its own turn**, so it stays a
   *  step where it happened rather than splitting the transcript. It does not
   *  rewrite the arming row either: two actions, two rows, even in one turn.
   *
   *  The row is self-contained because the event is: `EventWaitCanceled` carries
   *  what it stopped, exactly as a delivery carries what it matched. */
  it('keeps a stand-down as a step in the turn that stood it down', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      // A later turn entirely, hours after the subscription was armed.
      { type: 'MessageReceived', text: 'never mind, stop watching' },
      {
        type: 'EventWaitCanceled',
        wait_id: 'w1',
        cause: 'agent_stand_down',
        on: [{ event_type: 'ChangeProposed' }],
        reason: REASON,
      },
      { type: 'ResponseGenerated', text: 'Stopped.' },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([{ state: 'waiting' }]);
    expect(waits(exchangeResponseEvents(exchanges[1]))).toMatchObject([
      {
        wait_id: 'w1',
        state: 'canceled',
        cause: 'agent_stand_down',
        subscription: 'ChangeProposed',
        reason: REASON,
      },
    ]);
  });

  /** Neither a delivery nor an expiry gets one: both WAKE the thread, so each
   *  already reads as its own turn further down. A second row would report the
   *  same thing twice. */
  it.each(['EventWaitDelivered', 'EventWaitExpired'] as const)(
    'adds no row for a %s that lands in a later exchange',
    (type) => {
      const { map, id } = makeThread();
      insertEvents(map, id, [
        ...park,
        { type: 'ResponseGenerated', text: "I'll watch for that." },
        { type: 'MessageReceived', text: 'any news?' },
        type === 'EventWaitDelivered'
          ? { type, wait_id: 'w1', event_id: 'evt-9', event_type: 'ChangeProposed', payload: {}, matched_index: 0, was_attached: false }
          : { type, wait_id: 'w1', was_attached: false },
        { type: 'ResponseGenerated', text: 'Here it is.' },
      ] as ThreadEvent[]);

      expect(waits(exchangeResponseEvents(getExchanges(map, id)[1]))).toHaveLength(0);
    },
  );

  /** The sharpest form of "a stop never rewrites the arming row": both actions
   *  in ONE turn. The agent arms a watch, changes its mind and stands it down
   *  before the turn ends. That used to collapse into a single relabelled row
   *  saying only that it had been stopped, losing the arming entirely. */
  it('draws both rows when a turn arms a wait and stands it down again', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      {
        type: 'EventWaitCanceled',
        wait_id: 'w1',
        cause: 'agent_stand_down',
        on: [{ event_type: 'ChangeProposed' }],
        reason: REASON,
      },
      { type: 'ResponseGenerated', text: 'On reflection, nothing to watch.' },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(1);
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([
      { wait_id: 'w1', state: 'waiting', reason: REASON },
      { wait_id: 'w1', state: 'canceled', cause: 'agent_stand_down', reason: REASON },
    ]);
  });

  /** A pre-2026-08-07 `EventWaitCanceled` carries neither what it stopped nor
   *  why. It still renders (the alternative is the silence this fixes), and it
   *  names nothing rather than inventing a subscription. */
  it('renders a legacy stand-down with nothing to name', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      { type: 'MessageReceived', text: 'stop' },
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'agent_stand_down' },
    ] as ThreadEvent[]);

    expect(waits(exchangeResponseEvents(getExchanges(map, id)[1]))).toMatchObject([
      { state: 'canceled', subscription: '', reason: '' },
    ]);
  });
});
