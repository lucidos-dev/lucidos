/** One doer turn, several exchanges, and every step where it happened.
 *
 *  A caller's utterance opens a boundary wherever it lands, so a doer turn the
 *  caller talks across is split by boundaries it does not own. Each piece reads
 *  under the words it followed, which is when it happened (ADR 0201). Only one
 *  of the exchanges may report the turn finishing.
 *
 *  A tool RESULT is the exception, and it is pairing rather than placement: it
 *  completes a step row that already exists, so it follows its call.
 *
 *  Plan: `docs/plans/2026-09-16-the-clock-is-the-only-order.md`.
 */
import { describe, it, expect } from 'vitest';
import { ev, heard, put, said } from './call-fixtures';
import { computeExchanges, exchangeStatus, groupIntoExchanges, queuedFollowupRun, type StoredEvent } from '../thread-events';
import { makeThreadState } from './thread-events-helpers';
import type { Exchange } from '../thread-events';

const MSG = 'msg-1';
const TS = '2026-08-31T07:15:17Z';

/** A delegated question, then two more utterances said across the doer's turn
 *  while it worked. The talker fielded both itself. */
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

/** Every event type this exchange holds as a step, in order. */
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

  // The steps taken before the caller cut in stay with the question that
  // asked for them. The RESULT joins them from below the utterance, because a
  // step row and its outcome are one row.
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

  // These two route CHRONOLOGICALLY, and chronologically is exactly where they
  // belong: under the last thing said before them.
  it('files the chronological bookkeeping under the words it followed', () => {
    const events = theCall();
    put(events, 16, { type: 'TodoListWritten', items: [], request_event_id: MSG });
    put(events, 17, { type: 'BackgroundBashStarted', task_id: 'b1', command: 'sleep 1', timeout_secs: 60, started_at: TS });
    const exchanges = groupIntoExchanges(events);
    const last = exchanges[exchanges.length - 1];
    expect(last.userEvent.type).toBe('SpokenMessageReceived');
    expect(stepTypes(last)).toContain('TodoListWritten');
    expect(stepTypes(last)).toContain('BackgroundBashStarted');
    // And nothing reached back into the turn that had already moved on.
    expect(stepTypes(exchanges[1])).not.toContain('TodoListWritten');
  });

  // Everything the doer produced after an utterance reads under it. The answer
  // the caller heard lands there too, beside the written one.
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
    // The one the turn was in when it ended, which is the last thing said
    // before it finished.
    expect(holders[0]).toBe(exchanges[exchanges.length - 1]);
  });

  // Two `Done` badges for ONE turn is the failure this guards. The delegated
  // exchange reads `interrupted`, which draws the "↳" continuation arrow: its
  // turn carried on below, and saying Done there would claim it finished
  // where it stopped. Nothing is left working once the call has rung off.
  it('reports no exchange still working once the call has rung off', () => {
    const exchanges = groupIntoExchanges(theCall());
    const verdicts = exchanges.map((e, i) =>
      exchangeStatus(e, '', i === exchanges.length - 1, false, false, true),
    );
    expect(verdicts).toEqual(['done', 'interrupted', 'done', 'done']);
  });
});

/** The turn moves to the newest card, and its LIVE role moves with it.
 *
 *  A caller talking over a working doer opens the newest exchange, and the
 *  turn carries on inside it. It owns the streaming buffer and the "Working"
 *  badge there, because that is where the work is now showing. A spoken
 *  boundary that took no turn is still stepped over.
 */
describe('a caller talking over a running turn', () => {
  /** The call above, cut off while the doer is still working. */
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
    const { activeIndex } = queuedFollowupRun(exchanges, /* threadBusy */ true);
    expect(exchanges[activeIndex].userEvent.type).toBe('SpokenMessageReceived');
    expect(activeIndex).toBe(exchanges.length - 1);
  });

  it('keeps that turn reading as working rather than continued', () => {
    const exchanges = groupIntoExchanges(midTurn());
    const { activeIndex } = queuedFollowupRun(exchanges, true);
    const turn = exchanges[activeIndex];
    expect(exchangeStatus(turn, '', /* isLast */ true, false, false, false)).toBe('streaming');
  });

  it('still queues a typed follow-up behind the turn', () => {
    const events = midTurn();
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    const exchanges = groupIntoExchanges(events);
    const run = queuedFollowupRun(exchanges, true);
    // Behind the card the turn is showing in, which the caller's words moved.
    expect(exchanges[run.activeIndex].userEvent.type).toBe('SpokenMessageReceived');
    expect(run.queuedOrder.map(i => exchanges[i].userEvent._eventId)).toEqual(['msg-2']);
  });

  // The turn is active but no longer last, so the queued bubble may not follow
  // it up the transcript. It belongs at the bottom, under what was said after
  // it. `renderExchanges` anchors the group on the last non-queued exchange,
  // which this pins as the utterance rather than the turn.
  // A waiting message sits at the end of the array and renders at the end of
  // the transcript, but nothing has happened in it. The reply belongs to the
  // question it answers, and a step here would take the message out of the
  // queue: Stop could no longer hand it back.
  it('answers the utterance, not the message still waiting behind it', () => {
    const events = midTurn();
    put(events, 9, { type: 'MessageReceived', text: 'and the changelog?', mode: 'human', _eventId: 'msg-2' });
    put(events, 20, { type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'Nearly done.', interrupted: false });
    const exchanges = groupIntoExchanges(events);
    const utterance = exchanges.find(e => e.userEvent.type === 'SpokenMessageReceived')!;
    const waiting = exchanges.find(e => e.userEvent._eventId === 'msg-2')!;
    expect(stepTypes(utterance)).toEqual(['SpokenReplyGenerated', 'SpokenReplyGenerated']);
    expect(stepTypes(waiting)).toEqual([]);
    // Still a queued candidate, so Stop can still hand it back.
    const run = queuedFollowupRun(exchanges, true);
    expect(run.queuedOrder.map(i => exchanges[i].userEvent._eventId)).toEqual(['msg-2']);
  });

  // The same rule with no utterance to fall back on. Stepping over the waiting
  // message is not enough on its own: the open exchange IS that message, so a
  // row that finds nowhere else lands right back in it.
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

  // A Stop-waiting panel draws no body, so it can hold no row either. The walk
  // steps over it and over the waiting message, and finds the turn beneath.
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

  // A delegated utterance is stepless in the same window as a typed message
  // waiting its turn. It is not retractable, because that would offer to unsay
  // something. It does not take the live role either, because the loop has not
  // reached it: the turn already running keeps its stream and its badge.
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
    const run = queuedFollowupRun(exchanges, /* threadBusy */ true);
    expect(run.queuedOrder).toEqual([]);
    expect(exchanges[run.activeIndex].userEvent._eventId).toBe(MSG);
  });

  // A greeting said before the turn's first token has nowhere to go, so it
  // opens a boundary. It must not take the turn's ownership with it: the turn's
  // own park row routes chronologically, and would land under "Hi there".
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

  // A call files rows of its own into whatever is open, so "stepless" stops
  // meaning "the loop has not taken it" the moment anyone speaks. Judged that
  // way, one stall or one delegation hands the message the running turn.
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
    // The stall lands in it, which is where it was said.
    expect(stepTypes(second)).toEqual(['SpokenReplyGenerated']);
    // And it is still waiting on the loop, so the running turn keeps the role.
    expect(exchanges[queuedFollowupRun(exchanges, true).activeIndex].userEvent._eventId).toBe(MSG);
  });

  // The typed half of the same trap. A delegation marker folds into whatever
  // is open, and a queued message must not lose its place in the queue to one.
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

  // With no turn under it, the delegated question is what the reader waits on.
  // The live role follows the TURN, and the caller's words moved where the
  // turn is showing. Picked from the retractable list alone there would be no
  // candidate at all: speech is not retractable.
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
    const run = queuedFollowupRun(exchanges, /* threadBusy */ true);
    const active = exchanges[run.activeIndex];
    expect(active.userEvent.type).toBe('SpokenMessageReceived');
    expect(active.tookTheTurn).toBe(true);
  });

  // The retract filter has to match what the retract OFFERED, or a message the
  // reader took back survives and renders as a turn that crashed.
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
    // The turn moved into that later utterance, so the anchor IS the active
    // one. Either way the queued bubble renders under the newest speech.
    expect(anchor).toBeGreaterThanOrEqual(run.activeIndex);
    expect(exchanges[anchor].userEvent.type).toBe('SpokenMessageReceived');
  });
});

/** A delegated utterance whose injection lands after the caller spoke again.
 *
 *  The absorb re-anchors that message to the moment the loop picked it up, and
 *  a caller's utterance does NOT hold that move: the doer engages with the
 *  message now, so its panel and its reply belong below what was said while it
 *  waited. Holding it would leave `current` above the bottom, and the turn's
 *  own bookkeeping would land in the utterance and read as a crash.
 *
 *  What must hold either way is that the bottom of the transcript settles once
 *  the call ends, and that nothing is said twice.
 */
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
      ev(6, { type: 'UserPromptInjected', text: 'How long will the release take?', mode: 'engine', injected_message_id: MSG }),
      said(7, 'Still working on it.'),
      // A quarter of a minute later, which is a second remark rather than the
      // same breath cut in two.
      said(22, 'About ten minutes.'),
      ev(23, { type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 22 }),
    ]);
  }

  // One utterance in the way must not cost the move against every OTHER
  // boundary. The message still lands below the child card it was taken up
  // after, and above the words the caller said while it waited.
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
      ev(7, { type: 'UserPromptInjected', text: 'How long will the release take?', mode: 'engine', injected_message_id: MSG }),
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

  // Only the MOVE is held. The message still takes the open exchange, so the
  // turn's own bookkeeping lands on it, while the call's rows go to the bottom
  // by name.
  it('keeps the turn on the message and the speech at the bottom', () => {
    const exchanges = groupIntoExchanges(theCall());
    expect(stepTypes(exchanges[1])).toEqual(['UserPromptInjected']);
    expect(stepTypes(exchanges[2])).toEqual([
      'SpokenReplyGenerated',
      'SpokenReplyGenerated',
      'VoiceSessionEnded',
    ]);
  });

  // The absorb means the loop TOOK the message, so a turn ran and ended. Its
  // terminal routes back by request id, wherever the message now sits.
  it('settles the message once its turn ends', () => {
    const events = theCall();
    put(events, 10, { type: 'ResponseGenerated', text: 'About ten minutes.', request_event_id: MSG });
    const exchanges = groupIntoExchanges(events);
    const message = exchanges.find(e => e.userEvent._eventId === MSG)!;
    expect(stepTypes(message)).toContain('ResponseGenerated');
    expect(exchangeStatus(message, '', false, false, false, true)).toBe('done');
  });
});
