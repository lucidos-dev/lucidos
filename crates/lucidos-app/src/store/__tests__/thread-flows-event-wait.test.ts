import { beforeEach, describe, expect, it } from 'vitest';
import { getExchanges, getLabel, insertEvents, makeThread, resetSeqCounter, TS } from './thread-flows-helpers';
import { exchangeResponseEvents, exchangeStatus, type ThreadEvent } from '../thread-events';
import { getCollapsedVisibleEvents, hidesEarlierProse, isStepMechanics } from '../event-rendering';
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
  it('updates the original row when a build starts during another turn', () => {
    const { map, id } = makeThread();
    const expires_at = '2026-08-06T13:00:00Z';
    const background_timing = { state: 'running', deadline: '2026-08-06T12:55:00Z' } as const;
    insertEvents(map, id, [
      park[0],
      park[1],
      { ...park[2], reason: 'a build slot', background_timing: { state: 'queued' } } as ThreadEvent,
      { type: 'ResponseGenerated', text: 'Queued for a build slot.' },
      { type: 'MessageReceived', text: 'What else do you need?' },
      { type: 'EventWaitTimingUpdated', wait_id: 'w1', reason: REASON, expires_at, background_timing },
    ]);

    const exchanges = getExchanges(map, id);
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([
      { wait_id: 'w1', state: 'waiting', reason: REASON, expires_at, background_timing },
    ]);
    expect(waits(exchangeResponseEvents(exchanges[1]))).toEqual([]);
  });

  it('renders as ONE row, the event-wait line replacing the await_event tool step', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(waits(events)).toHaveLength(1);
    expect(steps(events)).toHaveLength(0);
    expect(waits(events)[0]).toMatchObject({ wait_id: 'w1', state: 'waiting', subscriptions: [{ event_type: 'ChangeProposed' }] });
  });

  it('keeps the arming time on the row through its resolution', () => {
    const armedAt = '2026-08-06T11:00:00Z';
    const { map, id } = makeThread();
    insertEvents(map, id, [
      park[0],
      park[1],
      { ...park[2], created: armedAt },
      { type: 'EventWaitExpired', wait_id: 'w1', created: '2026-08-06T12:00:00Z' },
    ] as Array<ThreadEvent & { created?: string }>);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(waits(events)).toMatchObject([{ state: 'timed_out', created: armedAt }]);
  });

  it('contributes no step mechanics when the park was the only action', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(events.filter(isStepMechanics)).toHaveLength(0);
    expect(waits(events)).toHaveLength(1);
  });

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
    expect(hidesEarlierProse(events)).toBe(true);
    expect(waits(getCollapsedVisibleEvents(events))).toMatchObject([
      { wait_id: 'w1', state: 'waiting', reason: REASON },
    ]);
  });

  it('reads as a finished step, not an in-progress one', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(events.some((e) => e.type === 'step' && e.outcome === 'pending')).toBe(false);
  });

  it('reports the turn as done rather than working', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, park);

    const exchange = getExchanges(map, id)[0];
    expect(exchangeStatus(exchange, '', true)).toBe('done');
    expect(getLabel(exchange)).toBe('Done');
  });

  it('reads as working when the agent works on after arming the wait', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'run the suite' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'lucidos await-event --on E2ELockReleased' }, tool_use_id: 'toolu_1' },
      ...park.slice(2),
      { type: 'CodingAgentToolResult', name: '', result: 'ALREADY HAPPENED, before this subscription existed.', tool_use_id: 'toolu_1' },
      { type: 'CodingAgentTextStreamed', text: 'The lock is free, so I retry now.' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: './scripts/e2e-api.sh' }, tool_use_id: 'toolu_2' },
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'agent_stand_down', reason: REASON },
      { type: 'CodingAgentToolCalled', name: 'TaskOutput', args: { block: true }, tool_use_id: 'toolu_3' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(exchangeStatus(exchange, '', true, false,  true,  false))
      .toBe('coding-agent-working');
  });

  it('reads as working from the moment the awaiting call answers', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      { type: 'MessageReceived', text: 'run the suite' },
      { type: 'CodingAgentToolCalled', name: 'Bash', args: { command: 'lucidos await-event --on E2ELockReleased' }, tool_use_id: 'toolu_1' },
      ...park.slice(2),
      { type: 'CodingAgentToolResult', name: '', result: 'ALREADY HAPPENED, before this subscription existed.', tool_use_id: 'toolu_1' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(exchangeStatus(exchange, '', true, false,  true,  false))
      .toBe('coding-agent-working');
  });

  it('goes back to working when an attached delivery lands', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'evt-9', event_type: 'ChangeProposed', payload: {}, matched_index: 0, created: '2026-04-17T01:00:00Z' },
      { type: 'ToolResult', name: 'await_event', result: 'ChangeProposed fired', created: '2026-04-17T01:00:00Z' },
      { type: 'ToolCalled', name: 'send_notification', args: { title: 'Build landed' }, created: '2026-04-17T01:00:00Z' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    const events = exchangeResponseEvents(exchange);
    expect(waits(events)).toHaveLength(1);
    expect(waits(events)[0]).toMatchObject({
      state: 'matched',
      created: TS,
      matched_at: '2026-04-17T01:00:00Z',
    });
    expect(steps(events)).toHaveLength(1);
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  it('never resolves an unrelated step with the await_event result', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'evt-9', event_type: 'ChangeProposed', payload: {}, matched_index: 0 },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      { type: 'ToolResult', name: 'await_event', result: 'ChangeProposed fired' },
    ] as ThreadEvent[]);

    const events = exchangeResponseEvents(getExchanges(map, id)[0]);
    expect(steps(events)).toHaveLength(1);
    expect(steps(events)[0]).toMatchObject({ outcome: 'pending' });
  });

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

  it('goes back to working when the deadline passes', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitExpired', wait_id: 'w1' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(waits(exchangeResponseEvents(exchange))[0]).toMatchObject({ state: 'timed_out' });
    expect(exchangeStatus(exchange, '', true)).toBe('streaming');
  });

  it('leaves the parked turn reading done when the user stops the wait', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' },
      { type: 'ToolResult', name: 'await_event', result: 'The wait was canceled.' },
    ] as ThreadEvent[]);

    const exchange = getExchanges(map, id)[0];
    expect(exchangeStatus(exchange, '', false, false, false,  true)).toBe('done');
  });

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
        actor: { kind: 'device', device_id: 'dev-1' },
      },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(2);
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([
      { state: 'waiting', reason: REASON },
    ]);
    expect(exchanges[1].userEvent.type).toBe('EventWaitCanceled');
    expect(exchangeResponseEvents(exchanges[1])).toHaveLength(0);
  });

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

  it('keeps a running turn owning its own work when a stop lands mid-flight', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      { type: 'MessageReceived', text: 'meanwhile, tidy the notes' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' } },
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop', reason: REASON },
      { type: 'TodoListWritten', items: [{ content: 'tidy', status: 'in_progress' }] },
      { type: 'BackgroundBashStarted', bash_id: 'b1', command: 'ls' },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges).toHaveLength(3);
    expect(exchanges[2].userEvent.type).toBe('EventWaitCanceled');
    expect(exchanges[2].steps).toHaveLength(0);
    expect(exchanges[1].steps.map((s) => s.event.type)).toEqual([
      'ToolCalled',
      'TodoListWritten',
      'BackgroundBashStarted',
    ]);
  });

  it('keeps a stand-down as a step in the turn that stood it down', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
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
        subscriptions: [{ event_type: 'ChangeProposed' }],
        reason: REASON,
      },
    ]);
  });

  it.each(['EventWaitDelivered', 'EventWaitExpired'] as const)(
    'adds no row for a %s that lands in a later exchange',
    (type) => {
      const { map, id } = makeThread();
      insertEvents(map, id, [
        ...park,
        { type: 'ResponseGenerated', text: "I'll watch for that." },
        { type: 'MessageReceived', text: 'any news?' },
        type === 'EventWaitDelivered'
          ? { type, wait_id: 'w1', event_id: 'evt-9', event_type: 'ChangeProposed', payload: {}, matched_index: 0 }
          : { type, wait_id: 'w1' },
        { type: 'ResponseGenerated', text: 'Here it is.' },
      ] as ThreadEvent[]);

      expect(waits(exchangeResponseEvents(getExchanges(map, id)[1]))).toHaveLength(0);
    },
  );

  it('files the work after a mid-turn delivery under the delivery, not above it', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      { type: 'MessageReceived', text: 'meanwhile, stop the build', event_id: 'req-1' },
      { type: 'ToolCalled', name: 'bash_kill', args: { task_id: 'b1' }, request_event_id: 'req-1' },
      { type: 'ToolResult', name: 'bash_kill', result: 'killed b1', request_event_id: 'req-1' },
      { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'del-1', event_type: 'ChangeProposed', payload: {}, matched_index: 0 },
      { type: 'PromptInjected', text: 'An event you subscribed to has arrived.', mode: 'agent', delivered_event_id: 'del-1' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' }, request_event_id: 'req-1' },
      { type: 'ToolResult', name: 'read_file', result: 'ok', request_event_id: 'req-1' },
      { type: 'ResponseGenerated', text: 'The change landed, and the build is stopped.', request_event_id: 'req-1' },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map((e) => e.userEvent.type)).toEqual([
      'MessageReceived',
      'MessageReceived',
      'PromptInjected',
    ]);
    expect(exchanges[1].steps.map((s) => s.event.type)).toEqual([
      'ToolCalled',
      'ToolResult',
      'EventWaitDelivered',
    ]);
    expect(exchanges[2].steps.map((s) => s.event.type)).toEqual([
      'ToolCalled',
      'ToolResult',
      'ResponseGenerated',
    ]);
  });

  it('leaves an idle delivery owning its own turn, arming row intact', () => {
    const { map, id } = makeThread('thread-1', 'running');
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      { type: 'EventWaitDelivered', wait_id: 'w1', event_id: 'del-1', event_type: 'ChangeProposed', payload: {}, matched_index: 0 },
      { type: 'PromptInjected', text: 'An event you subscribed to has arrived.', mode: 'agent', delivered_event_id: 'del-1', event_id: 'reentry-1' },
      { type: 'ToolCalled', name: 'read_file', args: { path: 'notes.md' }, request_event_id: 'reentry-1' },
      { type: 'ToolResult', name: 'read_file', result: 'ok', request_event_id: 'reentry-1' },
      { type: 'ResponseGenerated', text: 'The change landed.', request_event_id: 'reentry-1' },
    ] as ThreadEvent[]);

    const exchanges = getExchanges(map, id);
    expect(exchanges.map((e) => e.userEvent.type)).toEqual([
      'MessageReceived',
      'PromptInjected',
    ]);
    expect(exchanges[1].steps.map((s) => s.event.type)).toEqual([
      'ToolCalled',
      'ToolResult',
      'ResponseGenerated',
    ]);
    expect(waits(exchangeResponseEvents(exchanges[0]))).toMatchObject([
      { wait_id: 'w1', state: 'matched', reason: REASON },
    ]);
    expect(exchangeStatus(exchanges[0], '', false)).toBe('done');
  });

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

  it('renders a legacy stand-down with nothing to name', () => {
    const { map, id } = makeThread();
    insertEvents(map, id, [
      ...park,
      { type: 'ResponseGenerated', text: "I'll watch for that." },
      { type: 'MessageReceived', text: 'stop' },
      { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'agent_stand_down' },
    ] as ThreadEvent[]);

    expect(waits(exchangeResponseEvents(getExchanges(map, id)[1]))).toMatchObject([
      { state: 'canceled', subscriptions: [], reason: '' },
    ]);
  });
});
