/** The caller's bubble appears as they start speaking.
 *
 *  A transcript is folded from events, and the caller's utterance reaches it
 *  only once the talker has decided what to do with the words. The stretch
 *  before that drew nothing at all, which is what the *live utterance* row
 *  covers.
 *
 *  It is a row nothing on the engine will ever write, appended past the fold.
 *  So what these cases pin is mostly what it must NOT do: take a turn, take the
 *  queue, move an exchange, or make the thread read as running. See
 *  `docs/plans/2026-08-31-a-bubble-appears-as-the-caller-speaks.md`.
 */
import { describe, it, expect } from 'vitest';
import { heard, put } from './call-fixtures';
import { signal } from '@preact/signals';
import { createLiveUtteranceBridge, installLiveUtteranceRow, liveUtteranceId } from '../liveUtterance';
import { effectiveThreadStatus, threadMap } from '../store';
import { voiceCall } from '../voice';
import {
  activeExchangeIndex,
  computeExchanges,
  handleEvent,
  isLiveUtteranceRow,
  makeOptimisticThreadState,
  queuedFollowupRun,
  queuedMessagesFromExchanges,
  type LiveUtterance,
  type StoredEvent,
  type ThreadState,
} from '../thread-events';
import { CALL_IDLE, type CallState } from '../../voice/callState';

const THREAD = 'thread-1';
const MSG = 'msg-1';

const SPEAKING: LiveUtterance = {
  eventId: liveUtteranceId(THREAD, 1),
  count: 1,
  created: '2026-08-31T07:16:00Z',
};

/** A thread with a doer turn under way: the caller asked, and the agent is
 *  streaming its answer. */
function withADoerWorking(): ThreadState {
  const thread = makeOptimisticThreadState({
    id: THREAD,
    title: 'A call',
    channel: 'chat',
    initiator: 'user',
    eventsLoaded: true,
    status: 'running',
  });
  put(thread.events, 1, {
    type: 'MessageReceived',
    text: 'what is going on today',
    mode: 'human',
    channel: 'chat',
    voice_session_id: 'sess-1',
    _eventId: MSG,
  });
  put(thread.events, 2, { type: 'TextStreamed', text: 'Sixty commits.', request_event_id: MSG });
  put(thread.events, 3, { type: 'ToolCalled', name: 'read_file', args: {}, _eventId: 'tc-1', request_event_id: MSG });
  return thread;
}

describe('the row lands under everything, and moves nothing', () => {
  it('draws at the bottom while a doer turn is still working', () => {
    const quiet = computeExchanges(withADoerWorking());
    const thread = withADoerWorking();
    thread.liveUtterance = SPEAKING;
    const speaking = computeExchanges(thread);

    expect(speaking).toHaveLength(quiet.length + 1);
    expect(isLiveUtteranceRow(speaking[speaking.length - 1].userEvent)).toBe(true);
    expect(speaking[speaking.length - 1].steps).toEqual([]);
  });

  it('leaves the working turn every step it had', () => {
    const thread = withADoerWorking();
    thread.liveUtterance = SPEAKING;
    const [turn] = computeExchanges(thread);
    expect(turn.steps.map(s => s.event.type)).toEqual(['TextStreamed', 'ToolCalled']);
  });

  it('keys on its own id, so it is one node rather than a remount per frame', () => {
    const thread = withADoerWorking();
    thread.liveUtterance = SPEAKING;
    const row = computeExchanges(thread)[1];
    expect(row.userEvent._eventId).toBe(SPEAKING.eventId);
  });
});

/** The row is appended past every path through the fold. A re-anchor is the
 *  thing that would notice one inside it: it walks the exchanges looking for
 *  the caller's speech, and stops at the first it finds. */
describe('the fold cannot see it', () => {
  function aReanchoringCall(): ThreadState {
    const thread = makeOptimisticThreadState({
      id: THREAD,
      title: 'A call',
      channel: 'chat',
      initiator: 'user',
      eventsLoaded: true,
      status: 'running',
    });
    put(thread.events, 1, {
      type: 'MessageReceived',
      text: 'check the deploy',
      mode: 'human',
      channel: 'chat',
      _eventId: MSG,
    });
    const [, spoken] = heard(2, 'and the tests too');
    thread.events.set(2, spoken);
    put(thread.events, 3, {
      type: 'UserPromptInjected',
      text: 'check the deploy',
      mode: 'agent',
      injected_message_id: MSG,
    });
    return thread;
  }

  it('folds a re-anchoring call the same way with a caller mid-sentence', () => {
    const quiet = computeExchanges(aReanchoringCall());
    const thread = aReanchoringCall();
    thread.liveUtterance = SPEAKING;
    const speaking = computeExchanges(thread);
    expect(speaking.slice(0, -1)).toEqual(quiet);
  });
});

describe('the row holds no turn', () => {
  function speakingOverADoer(): ThreadState {
    const thread = withADoerWorking();
    thread.liveUtterance = SPEAKING;
    return thread;
  }

  it('leaves the live stream with the turn that is producing it', () => {
    const exchanges = computeExchanges(speakingOverADoer());
    expect(activeExchangeIndex(exchanges, /* busy */ true)).toBe(0);
  });

  it('is never the active exchange on an idle thread either', () => {
    const exchanges = computeExchanges(speakingOverADoer());
    expect(queuedFollowupRun(exchanges, /* busy */ false).activeIndex).toBe(0);
  });

  it('is offered for retract nowhere: nothing was said to unsay', () => {
    const exchanges = computeExchanges(speakingOverADoer());
    expect(queuedMessagesFromExchanges(exchanges, /* busy */ true)).toEqual([]);
  });

  /** Nothing is in flight behind a bubble that is only a pulse. Counted as a
   *  turn, it would pin the thread on Running for as long as somebody talks. */
  it('never makes the thread read as running', () => {
    const idle = makeOptimisticThreadState({
      id: THREAD,
      title: 'A call',
      channel: 'chat',
      initiator: 'user',
      eventsLoaded: true,
      status: 'idle',
    });
    expect(effectiveThreadStatus(idle)).toBe('idle');
    idle.liveUtterance = SPEAKING;
    expect(effectiveThreadStatus(idle)).toBe('idle');
  });
});

describe('the words replace the row', () => {
  function threadWithARow(): Map<string, ThreadState> {
    const thread = withADoerWorking();
    thread.liveUtterance = SPEAKING;
    return new Map([[THREAD, thread]]);
  }

  it('goes when the talker answered the caller alone', () => {
    const map = threadWithARow();
    handleEvent(map, THREAD, 9, { type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'and the tests' } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');
    expect(map.get(THREAD)?.liveUtterance).toBeNull();
  });

  it('goes when the talker delegated it instead', () => {
    const map = threadWithARow();
    handleEvent(map, THREAD, 9, {
      type: 'MessageReceived',
      text: 'and the tests',
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');
    expect(map.get(THREAD)?.liveUtterance).toBeNull();
  });

  /** The engine holds one utterance at a time, so a caller who barges in has a
   *  second row up before the first one's words arrive. Those words are the
   *  FIRST sentence's, and they say nothing about the one still being said. */
  it('leaves a newer row alone when an older utterance lands late', () => {
    const thread = withADoerWorking();
    const second: LiveUtterance = {
      eventId: liveUtteranceId(THREAD, 2),
      count: 2,
      created: '2026-08-31T07:16:02Z',
    };
    thread.liveUtterance = second;
    const map = new Map([[THREAD, thread]]);
    const spoken = (seq: number, text: string): void => {
      handleEvent(map, THREAD, seq, { type: 'SpokenMessageReceived', session_id: 'sess-1', text } as StoredEvent, `2026-08-31T07:16:0${seq}Z`, `e-${seq}`);
    };

    spoken(9, 'what is going on today');
    expect(map.get(THREAD)?.liveUtterance).toEqual(second);

    spoken(10, 'and the tests');
    expect(map.get(THREAD)?.liveUtterance).toBeNull();
  });

  /** A message somebody typed mid-call says nothing about what is being said
   *  out loud. The composer stays live during a call (ADR 0148). */
  it('stays for a typed message landing while the caller talks', () => {
    const map = threadWithARow();
    handleEvent(map, THREAD, 9, {
      type: 'MessageReceived',
      text: 'typed while talking',
      mode: 'human',
      channel: 'chat',
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');
    expect(map.get(THREAD)?.liveUtterance).toEqual(SPEAKING);
  });
});

describe('the bridge between a call and a thread', () => {
  function harness() {
    const call = signal<CallState>(CALL_IDLE);
    const drawn: { threadId: string; row: LiveUtterance }[] = [];
    const erased: string[] = [];
    let tick = 0;
    const bridge = createLiveUtteranceBridge({
      call,
      draw: (threadId, row) => drawn.push({ threadId, row }),
      erase: (threadId) => erased.push(threadId),
      now: () => `2026-08-31T07:16:0${tick++}Z`,
    });
    return { call, drawn, erased, bridge };
  }

  const speaking = (count: number): CallState => ({
    ...CALL_IDLE,
    phase: 'listening',
    threadId: THREAD,
    utterance: 'live',
    utteranceCount: count,
  });

  it('draws one row per utterance', () => {
    const h = harness();
    h.call.value = speaking(1);
    h.call.value = { ...speaking(1), utterance: 'landing' };
    expect(h.drawn).toHaveLength(1);
    h.call.value = speaking(2);
    expect(h.drawn.map(d => d.row.count)).toEqual([1, 2]);
  });

  it('erases it when the utterance is withdrawn', () => {
    const h = harness();
    h.call.value = speaking(1);
    h.call.value = { ...speaking(1), utterance: 'none' };
    expect(h.erased).toEqual([THREAD]);
  });

  /** The other end erases the row when the words land, and the utterance is
   *  still `transcribed` for a moment after. Anything else the call does then
   *  must not redraw a bubble over words the reader can already read. */
  it('does not redraw a row whose words have landed', () => {
    const h = harness();
    h.call.value = speaking(1);
    h.call.value = { ...speaking(1), utterance: 'transcribed' };
    h.call.value = { ...speaking(1), utterance: 'transcribed', phase: 'speaking' };
    expect(h.drawn).toHaveLength(1);
  });

  it('takes the row with it when the call ends', () => {
    const h = harness();
    h.call.value = speaking(1);
    h.call.value = CALL_IDLE;
    expect(h.erased).toEqual([THREAD]);
  });

  it('stops watching once disposed', () => {
    const h = harness();
    h.bridge.dispose();
    h.call.value = speaking(1);
    expect(h.drawn).toEqual([]);
  });
});

/** The whole seam, end to end: the live call signal, the real writer, and the
 *  thread map the transcript reads. */
describe('the live wiring', () => {
  it('draws the row on the thread the call is running on', () => {
    installLiveUtteranceRow();
    threadMap.value = new Map([[THREAD, withADoerWorking()]]);
    voiceCall.value = {
      ...CALL_IDLE,
      phase: 'listening',
      threadId: THREAD,
      utterance: 'live',
      utteranceCount: 1,
    };
    expect(threadMap.value.get(THREAD)?.liveUtterance?.count).toBe(1);

    voiceCall.value = CALL_IDLE;
    expect(threadMap.value.get(THREAD)?.liveUtterance).toBeNull();
  });

  /** A second call counts from one again, so the tally of landed words starts
   *  over with it. Left standing, it would clear the new call's first row on
   *  the first thing anybody said. */
  it('starts the tally over for a fresh call', () => {
    installLiveUtteranceRow();
    const thread = withADoerWorking();
    thread.settledUtterances = 3;
    threadMap.value = new Map([[THREAD, thread]]);
    voiceCall.value = {
      ...CALL_IDLE,
      phase: 'listening',
      threadId: THREAD,
      utterance: 'live',
      utteranceCount: 1,
    };
    expect(thread.settledUtterances).toBe(0);
    expect(thread.liveUtterance?.count).toBe(1);
    voiceCall.value = CALL_IDLE;
  });
});
