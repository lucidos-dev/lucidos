import { describe, it, expect } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import { computeExchanges, exchangeStatus, groupIntoExchanges, queuedFollowupRun, type StoredEvent } from '../thread-events';
import { makeThreadState } from './thread-events-helpers';
import type { Exchange } from '../thread-events';

const MSG = 'msg-1';
const TS = '2026-08-31T07:15:17Z';

function theCall(): Map<number, StoredEvent> {
  return new Map([
    ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
    said(2, 'Hi there. How can I help?'),
    ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
    ev(4, {
      type: 'MessageReceived',
      text: 'How long will the release take?',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
      _eventId: MSG,
    }),
    ev(5, { type: 'ThoughtStreamed', text: '', request_event_id: MSG }),
    ev(6, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-1', request_event_id: MSG }),
    heard(7, 'Are you still there?'),
    said(8, 'Still working on it.'),
    ev(9, { type: 'ToolResult', name: 'run_bash', result: 'ok', tool_called_event_id: 'tc-1', request_event_id: MSG }),
    heard(10, 'Any idea how long?'),
    said(11, 'Almost there.'),
    ev(12, { type: 'TextStreamed', text: 'About ten minutes.', request_event_id: MSG }),
    ev(13, { type: 'ResponseGenerated', text: 'About ten minutes.', request_event_id: MSG }),
    said(14, 'The release will take about ten minutes.'),
    ev(15, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 44 }),
  ]);
}

function stepTypes(exchange: Exchange): string[] {
  return exchange.steps.map(s => s.event.type);
}

const TERMINALS: ReadonlySet<string> = new Set([
  'ResponseGenerated',
  'ResponseCanceled',
  'ResponseAborted',
  'ResponseFailed',
]);

describe('a doer turn split across several exchanges', () => {
  it('opens one exchange per utterance, in the order they were said', () => {
    expect(groupIntoExchanges(theCall()).map(e => e.userEvent.type)).toEqual([
      'SpokenReplyGenerated',
      'MessageReceived',
      'SpokenMessageReceived',
      'SpokenMessageReceived',
    ]);
  });

  it('keeps the steps taken before the caller spoke, and their results', () => {
    const exchanges = groupIntoExchanges(theCall());
    const delegated = exchanges[1];
    expect(delegated.userEvent.type).toBe('MessageReceived');
    expect(stepTypes(delegated)).toEqual([
      'ThoughtStreamed',
      'ToolCalled',
      'ToolResult',
    ]);
  });

  it('files the chronological bookkeeping under the words it followed', () => {
    const events = theCall();
    put(events, 16, { type: 'TodoListWritten', items: [], request_event_id: MSG });
    put(events, 17, { type: 'BackgroundBashStarted', task_id: 'b1', command: 'sleep 1', timeout_secs: 60, started_at: TS });
    const exchanges = groupIntoExchanges(events);
    const last = exchanges[exchanges.length - 1];
    expect(last.userEvent.type).toBe('SpokenMessageReceived');
    expect(stepTypes(last)).toContain('TodoListWritten');
    expect(stepTypes(last)).toContain('BackgroundBashStarted');
    expect(stepTypes(exchanges[1])).not.toContain('TodoListWritten');
  });

  it('gives each utterance the work that followed it', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(stepTypes(exchanges[2])).toEqual(['SpokenReplyGenerated']);
    expect(stepTypes(exchanges[3])).toEqual([
      'SpokenReplyGenerated',
      'TextStreamed',
      'ResponseGenerated',
      'SpokenReplyGenerated',
      'VoiceSessionEnded',
    ]);
  });

  it('settles the turn on exactly one exchange', () => {
    const exchanges = groupIntoExchanges(theCall());
    const holders = exchanges.filter(e => stepTypes(e).some(t => TERMINALS.has(t)));
    expect(holders).toHaveLength(1);
    expect(holders[0]).toBe(exchanges[exchanges.length - 1]);
  });

  it('reports no exchange still working once the call has rung off', () => {
    const exchanges = groupIntoExchanges(theCall());
    const verdicts = exchanges.map((e, i) =>
      exchangeStatus(e, '', i === exchanges.length - 1, false, false, true),
    );
    expect(verdicts).toEqual(['done', 'interrupted', 'done', 'done']);
  });
});

describe('a caller talking over a running turn', () => {
  function midTurn(): Map<number, StoredEvent> {
    const events = new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
      ev(4, {
        type: 'MessageReceived',
        text: 'How long will the release take?',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      ev(5, { type: 'ThoughtStreamed', text: '', request_event_id: MSG }),
      ev(6, { type: 'ToolCalled', name: 'run_bash', args: {}, _eventId: 'tc-1', request_event_id: MSG }),
      heard(7, 'Are you still there?'),
      said(8, 'Still working on it.'),
    ]);
    return events;
  }

  it('makes the utterance the running turn carried into the active one', () => {
    const exchanges = groupIntoExchanges(midTurn());
    const { activeIndex } = queuedFollowupRun(exchanges,  true);
    expect(exchanges[activeIndex].userEvent.type).toBe('SpokenMessageReceived');
    expect(activeIndex).toBe(exchanges.length - 1);
  });

  it('keeps that turn reading as working rather than continued', () => {
    const exchanges = groupIntoExchanges(midTurn());
    const { activeIndex } = queuedFollowupRun(exchanges, true);
    const turn = exchanges[activeIndex];
    expect(exchangeStatus(turn, '',  true, false, false, false)).toBe('streaming');
  });

  it('still queues a typed follow-up behind the turn', () => {
    const events = midTurn();
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    const exchanges = groupIntoExchanges(events);
    const run = queuedFollowupRun(exchanges, true);
    expect(exchanges[run.activeIndex].userEvent.type).toBe('SpokenMessageReceived');
    expect(run.queuedOrder.map(i => exchanges[i].userEvent._eventId)).toEqual(['msg-2']);
  });

  it('answers the utterance, not the message still waiting behind it', () => {
    const events = midTurn();
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 20, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'Nearly done.', interrupted: false });
    const exchanges = groupIntoExchanges(events);
    const utterance = exchanges.find(e => e.userEvent.type === 'SpokenMessageReceived')!;
    const waiting = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    expect(stepTypes(utterance)).toEqual(['SpokenReplyGenerated', 'SpokenReplyGenerated']);
    expect(stepTypes(waiting)).toEqual([]);
    const run = queuedFollowupRun(exchanges, true);
    expect(run.queuedOrder.map(i => exchanges[i].userEvent._eventId)).toEqual(['msg-2']);
  });

  it('answers the running turn when the caller has not spoken since', () => {
    const events = midTurn();
    events.delete(7);
    events.delete(8);
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 10, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'Nearly done.', interrupted: false });
    put(events, 11, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 20 });
    const exchanges = groupIntoExchanges(events);
    const turn = exchanges.find(e => e.userEvent._eventId === MSG)!;
    const waiting = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    expect(stepTypes(turn)).toContain('SpokenReplyGenerated');
    expect(stepTypes(waiting)).toEqual([]);
    const run = queuedFollowupRun(exchanges, true);
    expect(run.queuedOrder.map(i => exchanges[i].userEvent._eventId)).toEqual(['msg-2']);
  });

  it('steps over a Stop-waiting panel as well', () => {
    const events = midTurn();
    events.delete(7);
    events.delete(8);
    put(events, 9, { type: 'EventWaitStarted', wait_id: 'w1', tool_use_id: 'tu-1', on: [], reason: 'the build', expires_at: TS, watermark: 0, request_event_id: MSG });
    put(events, 10, { type: 'EventWaitCanceled', wait_id: 'w1', cause: 'user_stop' });
    put(events, 11, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 12, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'Nearly done.', interrupted: false });
    const exchanges = groupIntoExchanges(events);
    const turn = exchanges.find(e => e.userEvent._eventId === MSG)!;
    const waiting = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    const stop = exchanges.find(e => e.userEvent.type === 'EventWaitCanceled')!;
    expect(stepTypes(turn)).toContain('SpokenReplyGenerated');
    expect(stepTypes(waiting)).toEqual([]);
    expect(stepTypes(stop)).toEqual([]);
  });

  it('never offers to retract a delegated utterance', () => {
    const events = midTurn();
    events.delete(7);
    events.delete(8);
    put(events, 9, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'And the changelog.' });
    put(events, 10, {
      type: 'MessageReceived',
      text: 'and the changelog?',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
      _eventId: 'msg-2',
    });
    const exchanges = groupIntoExchanges(events);
    const run = queuedFollowupRun(exchanges,  true);
    expect(run.queuedOrder).toEqual([]);
    expect(exchanges[run.activeIndex].userEvent._eventId).toBe(MSG);
  });

  it('lets a greeting open a boundary without taking the running turn', () => {
    const events = new Map([
      ev(1, { type: 'MessageReceived', text: 'watch the build', mode: 'human', _eventId: MSG }),
      ev(2, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(3, 'Hi there. How can I help?'),
      ev(4, {
        type: 'EventWaitStarted',
        wait_id: 'w1',
        tool_use_id: 'tu-1',
        on: [],
        reason: 'the build',
        expires_at: TS,
        watermark: 0,
        request_event_id: MSG,
      }),
    ]);
    const exchanges = groupIntoExchanges(events);
    expect(exchanges.map(e => e.userEvent.type)).toEqual(['MessageReceived', 'SpokenReplyGenerated']);
    expect(stepTypes(exchanges[0])).toEqual(['EventWaitStarted']);
    expect(stepTypes(exchanges[1])).toEqual([]);
  });

  it('is not fooled into ingesting a message by a call row landing in it', () => {
    const events = midTurn();
    events.delete(7);
    events.delete(8);
    put(events, 9, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'And the changelog.' });
    put(events, 10, {
      type: 'MessageReceived',
      text: 'and the changelog?',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
      _eventId: 'msg-2',
    });
    put(events, 11, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'Checking.', interrupted: false });
    const exchanges = groupIntoExchanges(events);
    const second = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    expect(stepTypes(second)).toEqual(['SpokenReplyGenerated']);
    expect(exchanges[queuedFollowupRun(exchanges, true).activeIndex].userEvent._eventId).toBe(MSG);
  });

  it('keeps a typed message queued when a delegation lands in it', () => {
    const events = midTurn();
    events.delete(7);
    events.delete(8);
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 10, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check it.' });
    const exchanges = groupIntoExchanges(events);
    const run = queuedFollowupRun(exchanges, true);
    expect(run.queuedOrder.map(i => exchanges[i].userEvent._eventId)).toEqual(['msg-2']);
    expect(exchanges[run.activeIndex].userEvent._eventId).toBe(MSG);
  });

  it('waits on the card the delegated question carried into', () => {
    const events = new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
      ev(4, {
        type: 'MessageReceived',
        text: 'How long will the release take?',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      heard(5, 'Are you still there?'),
    ]);
    const exchanges = groupIntoExchanges(events);
    const run = queuedFollowupRun(exchanges,  true);
    const active = exchanges[run.activeIndex];
    expect(active.userEvent.type).toBe('SpokenMessageReceived');
    expect(active.tookTheTurn).toBe(true);
  });

  it('removes a retracted message a delegation marker had landed in', () => {
    const events = midTurn();
    events.delete(7);
    events.delete(8);
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 10, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check it.' });
    put(events, 11, { type: 'QueuedMessageRemoved', removed_message_id: 'msg-2' });
    const thread = makeThreadState();
    thread.events = new Map(events);
    const exchanges = computeExchanges(thread);
    expect(exchanges.some(e => e.userEvent._eventId === 'msg-2')).toBe(false);
  });

  it('leaves the queued follow-up below the later utterance', () => {
    const events = midTurn();
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 10, { type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'Still going?' });
    const exchanges = groupIntoExchanges(events);
    const run = queuedFollowupRun(exchanges, true);
    let anchor = run.activeIndex;
    for (let i = exchanges.length - 1; i >= 0; i--) {
      if (!run.queuedIndices.has(i)) { anchor = i; break; }
    }
    expect(anchor).toBeGreaterThanOrEqual(run.activeIndex);
    expect(exchanges[anchor].userEvent.type).toBe('SpokenMessageReceived');
  });
});

describe('an injection absorbed after the caller spoke', () => {
  function theCall(): Map<number, StoredEvent> {
    return new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
      ev(4, {
        type: 'MessageReceived',
        text: 'How long will the release take?',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      heard(5, 'Are you still there?'),
      ev(6, { type: 'PromptInjected', text: 'How long will the release take?', mode: 'engine', injected_message_id: MSG }),
      said(7, 'Still working on it.'),
      said(22, 'About ten minutes.'),
      ev(23, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 22 }),
    ]);
  }

  it('re-anchors below a card it was taken up after, and no further', () => {
    const events = new Map([
      ev(1, { type: 'VoiceSessionStarted', session_id: 'sess-1' }),
      said(2, 'Hi there. How can I help?'),
      ev(3, { type: 'WorkDelegated', session_id: 'sess-1', reason: 'Check the release.' }),
      ev(4, {
        type: 'MessageReceived',
        text: 'How long will the release take?',
        mode: 'human',
        channel: 'chat',
        voice_session_id: 'sess-1',
        _eventId: MSG,
      }),
      ev(5, {
        type: 'ChildThreadCompleted',
        child_thread_id: 'child-1',
        child_thread_title: 'The release check',
        status: 'success',
        summary: 'Done.',
      }),
      heard(6, 'Are you still there?'),
      ev(7, { type: 'PromptInjected', text: 'How long will the release take?', mode: 'engine', injected_message_id: MSG }),
    ]);
    expect(groupIntoExchanges(events).map(e => e.userEvent.type)).toEqual([
      'SpokenReplyGenerated',
      'ChildThreadCompleted',
      'MessageReceived',
      'SpokenMessageReceived',
    ]);
  });

  it('holds the re-anchor above the utterance said while it waited', () => {
    expect(groupIntoExchanges(theCall()).map(e => e.userEvent.type)).toEqual([
      'SpokenReplyGenerated',
      'MessageReceived',
      'SpokenMessageReceived',
    ]);
  });

  it('keeps every line the call produced, once each', () => {
    const exchanges = groupIntoExchanges(theCall());
    const spoken: string[] = [];
    for (const exchange of exchanges) {
      const starter = exchange.userEvent as { type: string; text?: string };
      if (starter.type === 'SpokenReplyGenerated' || starter.type === 'SpokenMessageReceived') {
        spoken.push(starter.text ?? '');
      }
      for (const s of exchange.steps) {
        if (s.event.type === 'SpokenReplyGenerated') spoken.push((s.event as { text: string }).text);
      }
    }
    expect(spoken).toEqual([
      'Hi there. How can I help?',
      'Are you still there?',
      'Still working on it.',
      'About ten minutes.',
    ]);
  });

  it('keeps the turn on the message and the speech at the bottom', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(stepTypes(exchanges[1])).toEqual(['PromptInjected']);
    expect(stepTypes(exchanges[2])).toEqual([
      'SpokenReplyGenerated',
      'SpokenReplyGenerated',
      'VoiceSessionEnded',
    ]);
  });

  it('settles the message once its turn ends', () => {
    const events = theCall();
    put(events, 10, { type: 'ResponseGenerated', text: 'About ten minutes.', request_event_id: MSG });
    const exchanges = groupIntoExchanges(events);
    const message = exchanges.find(e => e.userEvent._eventId === MSG)!;
    expect(stepTypes(message)).toContain('ResponseGenerated');
    expect(exchangeStatus(message, '', false, false, false, true)).toBe('done');
  });
});
