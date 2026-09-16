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
  isLiveReplyRow,
  isLiveUtteranceRow,
  makeOptimisticThreadState,
  queuedFollowupRun,
  queuedMessagesFromExchanges,
  type LiveReply,
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
    thread.liveUtterances = [SPEAKING];
    const speaking = computeExchanges(thread);

    expect(speaking).toHaveLength(quiet.length + 1);
    expect(isLiveUtteranceRow(speaking[speaking.length - 1].userEvent)).toBe(true);
    expect(speaking[speaking.length - 1].steps).toEqual([]);
  });

  it('leaves the working turn every step it had', () => {
    const thread = withADoerWorking();
    thread.liveUtterances = [SPEAKING];
    const [turn] = computeExchanges(thread);
    expect(turn.steps.map(s => s.event.type)).toEqual(['TextStreamed', 'ToolCalled']);
  });

  it('keys on its own id, so it is one node rather than a remount per frame', () => {
    const thread = withADoerWorking();
    thread.liveUtterances = [SPEAKING];
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
    thread.liveUtterances = [SPEAKING];
    const speaking = computeExchanges(thread);
    expect(speaking.slice(0, -1)).toEqual(quiet);
  });
});

describe('the row holds no turn', () => {
  function speakingOverADoer(): ThreadState {
    const thread = withADoerWorking();
    thread.liveUtterances = [SPEAKING];
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
    idle.liveUtterances = [SPEAKING];
    expect(effectiveThreadStatus(idle)).toBe('idle');
  });
});

/** A row is claimed on the WORDS it carries, never on its count. The count is
 *  the browser's tally of utterances and a persisted row is the provider's,
 *  and one transcription item can hold two of the browser's. */
describe('the words replace the row', () => {
  /** A row at `count` carrying the caller's finished words. */
  const worded = (count: number, text: string): LiveUtterance => ({
    eventId: liveUtteranceId(THREAD, count),
    count,
    created: `2026-08-31T07:16:0${count}Z`,
    text,
  });

  function threadWithARow(rows: LiveUtterance[] = [worded(1, 'and the tests')]): Map<string, ThreadState> {
    const thread = withADoerWorking();
    thread.liveUtterances = rows;
    return new Map([[THREAD, thread]]);
  }

  const spoken = (map: Map<string, ThreadState>, seq: number, text: string): void => {
    handleEvent(map, THREAD, seq, { type: 'SpokenMessageReceived', session_id: 'sess-1', text } as StoredEvent, `2026-08-31T07:16:0${seq}Z`, `e-${seq}`);
  };

  const delegated = (map: Map<string, ThreadState>, seq: number, text: string): void => {
    handleEvent(map, THREAD, seq, {
      type: 'MessageReceived',
      text,
      mode: 'human',
      channel: 'chat',
      voice_session_id: 'sess-1',
    } as StoredEvent, `2026-08-31T07:16:0${seq}Z`, `e-${seq}`);
  };

  it('goes when the talker answered the caller alone', () => {
    const map = threadWithARow();
    spoken(map, 9, 'and the tests');
    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
  });

  it('goes when the talker delegated it instead', () => {
    const map = threadWithARow();
    delegated(map, 9, 'and the tests');
    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
  });

  /** The engine holds one utterance at a time, so a caller who barges in has a
   *  second row up before the first one's words arrive. That row carries no
   *  words yet, and the landing ones are the FIRST sentence's. */
  it('leaves a wordless newer row alone when an older utterance lands', () => {
    const speaking: LiveUtterance = {
      eventId: liveUtteranceId(THREAD, 2),
      count: 2,
      created: '2026-08-31T07:16:02Z',
    };
    const map = threadWithARow([worded(1, 'what is going on today'), speaking]);

    spoken(map, 9, 'what is going on today');
    expect(map.get(THREAD)?.liveUtterances).toEqual([speaking]);
  });

  /**
   * The defect this whole ledger replaced, replayed from the real call.
   *
   * The browser's gate closed on a pause and counted two utterances. The
   * provider folded both into ONE transcription item, so the engine wrote a
   * single row. Matched by count, that row cleared only the counts at or below
   * one. The worded row at count 2 then stood for the rest of the call,
   * painting the same sentence a second time.
   */
  it('goes when the caller counted two utterances and the engine wrote one row', () => {
    const merged = 'Hello Hmm The transcript isn\'t really working.';
    const map = threadWithARow([worded(2, merged)]);

    delegated(map, 9, merged);

    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
  });

  /**
   * The engine can write TWO rows against ONE live row.
   *
   * `onSpeech` reaches the reducer on a gate EDGE, so a caller who does not
   * pause for 320 ms holds one row across two provider turns. `call.rs` flushes
   * the first utterance when the second arrives, so two rows land. A count
   * claims the row on the FIRST and is left owed one. Every later bubble is
   * then retired the instant it gains words, which is the blank ADR 0174
   * removes.
   */
  it('is claimed by its OWN words when the engine wrote two rows for it', () => {
    const first = 'what is going on today';
    const second = 'and the tests too';
    // The row holds the SECOND transcript: the first landed while the gate was
    // still open, and a live utterance takes no caption.
    const map = threadWithARow([worded(1, second)]);

    spoken(map, 9, first);
    expect(map.get(THREAD)?.liveUtterances?.[0].text).toBe(second);

    spoken(map, 10, second);
    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
  });

  /**
   * The engine can write NO row at all.
   *
   * Words the caller spent answering a question card ARE the answer's row, so
   * `call.rs` drops them (`Resolution::SettledWithTheirWords`). The live row
   * for them is an orphan, and a count would hand it to the next utterance's
   * row. The `VoiceSessionEnded` sweep is what retires it.
   */
  it('leaves an orphan alone and claims the row that was written', () => {
    const answered = worded(1, 'the second one');
    const asked = worded(2, 'and run the tests');
    const map = threadWithARow([answered, asked]);

    spoken(map, 9, 'and run the tests');
    expect(map.get(THREAD)?.liveUtterances).toEqual([answered]);

    handleEvent(map, THREAD, 10, {
      type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 30,
    } as StoredEvent, '2026-08-31T07:16:10Z', 'e-10');
    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
  });

  /** `doer.rs::wake` trims before it writes, so a delegated row's text is the
   *  trimmed transcript while the client holds the frame's own. Both sides are
   *  trimmed for the match, which is the one normalization. */
  it('claims a delegated row whose text the engine trimmed', () => {
    const map = threadWithARow([worded(1, ' and the tests ')]);
    delegated(map, 9, 'and the tests');
    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
  });

  /** `call.rs` emits the persisted row BEFORE it sends `user_turn_ended`, and
   *  the two travel on different transports. Driven through the real bridge,
   *  because the claim runs through `writeRow` rather than being called. */
  it('claims when the engine\'s row arrives before the words', () => {
    installLiveUtteranceRow();
    const thread = withADoerWorking();
    threadMap.value = new Map([[THREAD, thread]]);
    const speaking = { ...CALL_IDLE, phase: 'listening' as const, threadId: THREAD, utteranceCount: 1 };

    voiceCall.value = { ...speaking, utterance: 'live' };
    expect(thread.liveUtterances).toHaveLength(1);

    // SSE wins the race: the row lands while the bubble is still a pulse.
    handleEvent(threadMap.value, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'and the tests',
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');
    expect(thread.liveUtterances).toHaveLength(1);
    expect(thread.unclaimedUtterances).toEqual(['and the tests']);

    voiceCall.value = { ...speaking, utterance: 'transcribed', heard: 'and the tests' };
    expect(thread.liveUtterances).toEqual([]);
    expect(thread.unclaimedUtterances).toEqual([]);
    voiceCall.value = CALL_IDLE;
  });

  /** A message somebody typed mid-call says nothing about what is being said
   *  out loud. The composer stays live during a call (ADR 0148). */
  it('stays for a typed message landing while the caller talks', () => {
    const row = worded(1, 'and the tests');
    const map = threadWithARow([row]);
    handleEvent(map, THREAD, 9, {
      type: 'MessageReceived',
      text: 'typed while talking',
      mode: 'human',
      channel: 'chat',
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');
    expect(map.get(THREAD)?.liveUtterances).toEqual([row]);
  });
});

describe('the bridge between a call and a thread', () => {
  function harness() {
    const call = signal<CallState>(CALL_IDLE);
    const drawn: { threadId: string; row: LiveUtterance }[] = [];
    const replies: { threadId: string; row: LiveReply }[] = [];
    const erased: string[] = [];
    let tick = 0;
    const bridge = createLiveUtteranceBridge({
      call,
      draw: (threadId, row) => drawn.push({ threadId, row }),
      erase: (threadId) => erased.push(threadId),
      drawReply: (threadId, row) => {
        replies.push({ threadId, row });
        return true;
      },
      now: () => `2026-08-31T07:16:0${tick++}Z`,
    });
    return { call, drawn, replies, erased, bridge };
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
    expect(threadMap.value.get(THREAD)?.liveUtterances?.[0].count).toBe(1);

    voiceCall.value = CALL_IDLE;
    expect(threadMap.value.get(THREAD)?.liveUtterances).toEqual([]);
  });

  /** A second call counts its utterances from one again, so the ledger starts
   *  over with them. A debt left over from the last call would claim the new
   *  one's first row the instant it got any words. */
  it('starts the ledger over for a fresh call', () => {
    installLiveUtteranceRow();
    const thread = withADoerWorking();
    thread.unclaimedUtterances = ['last call'];
    thread.liveReply = { eventId: 'stale', created: '2026-08-31T07:00:00Z', text: 'last call' };
    threadMap.value = new Map([[THREAD, thread]]);
    voiceCall.value = {
      ...CALL_IDLE,
      phase: 'listening',
      threadId: THREAD,
      utterance: 'live',
      utteranceCount: 1,
    };
    expect(thread.unclaimedUtterances).toEqual([]);
    expect(thread.liveReply).toBeUndefined();
    expect(thread.liveUtterances?.[0].count).toBe(1);
    voiceCall.value = CALL_IDLE;
  });

  /** A claimed row stays gone. The bridge still holds the words it drew, so a
   *  provider revising its own transcript rewrites a row the engine has
   *  already written down. Putting it back paints the sentence twice. */
  it('does not put a claimed row back when the provider revises it', () => {
    installLiveUtteranceRow();
    const thread = withADoerWorking();
    threadMap.value = new Map([[THREAD, thread]]);
    const speaking = { ...CALL_IDLE, phase: 'listening' as const, threadId: THREAD, utteranceCount: 1 };

    voiceCall.value = { ...speaking, utterance: 'live' };
    voiceCall.value = { ...speaking, utterance: 'transcribed', heard: 'fix the blank thread' };
    expect(thread.liveUtterances).toHaveLength(1);

    handleEvent(threadMap.value, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'fix the blank thread',
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');
    expect(thread.liveUtterances).toEqual([]);

    voiceCall.value = { ...speaking, utterance: 'transcribed', heard: 'fix the blank thread view' };
    expect(thread.liveUtterances).toEqual([]);
    voiceCall.value = CALL_IDLE;
  });
});

/**
 * The talker's own row, drawn while it speaks.
 *
 * The client is sent every word of a reply as it is said, and dropped them all:
 * the transcript held nothing from the first word until the engine's row for
 * the whole reply landed. On one 136-second call that row arrived at hangup,
 * so the transcript said nothing for the length of the call.
 */
describe('the talker is drawn while it speaks', () => {
  const speaking = (said: string, count = 1): CallState => ({
    ...CALL_IDLE,
    phase: 'speaking',
    threadId: THREAD,
    said,
    replyCount: count,
  });

  /** `claimed` makes the store answer as it does once the engine's own row has
   *  landed: there is nothing standing for a rewrite to find. */
  function bridged(claimed: () => boolean = () => false) {
    const call = signal<CallState>(CALL_IDLE);
    const replies: { threadId: string; row: LiveReply; fresh: boolean }[] = [];
    let tick = 0;
    const bridge = createLiveUtteranceBridge({
      call,
      draw: () => {},
      erase: () => {},
      drawReply: (threadId, row, fresh) => {
        if (!fresh && claimed()) return false;
        replies.push({ threadId, row, fresh });
        return true;
      },
      now: () => `2026-08-31T07:16:0${tick++}Z`,
    });
    return { call, replies, bridge };
  }

  it('rewrites one row as the words arrive', () => {
    const h = bridged();
    h.call.value = speaking('Hey');
    h.call.value = speaking('Hey! How can');
    expect(h.replies.map(r => r.row.text)).toEqual(['Hey', 'Hey! How can']);
    // One row, so the reply grows in place rather than stacking.
    expect(new Set(h.replies.map(r => r.row.eventId)).size).toBe(1);
  });

  /** **The restarting-bubble regression.** A Live talker paces its transcript
   *  with its audio, so a reply has 700 ms holes inside it and `said` empties
   *  at every one. A row per hole is four bubbles where `call.rs` writes one,
   *  and the reader watches sentences disappear as they are spoken. */
  it('joins the stretches of one reply into one growing bubble', () => {
    const h = bridged();
    h.call.value = speaking('One: only you from the UI.');
    // The pause: `said` empties, and the bridge is not asked to draw.
    h.call.value = { ...speaking(''), replyCount: 1 };
    h.call.value = speaking(' Two: also a CLI verb.', 2);
    h.call.value = speaking(' Two: also a CLI verb, which is riskier.', 2);

    expect(h.replies[h.replies.length - 1].row.text).toBe(
      'One: only you from the UI. Two: also a CLI verb, which is riskier.',
    );
    // And it is the same bubble throughout, so nothing on screen is replaced.
    expect(new Set(h.replies.map(r => r.row.eventId)).size).toBe(1);
  });

  /** A reply already on screen must not have its timestamp jump as it grows,
   *  through a revision and through a pause alike. */
  it('keeps the moment the bubble went up', () => {
    const h = bridged();
    h.call.value = speaking('Hey');
    h.call.value = speaking('Hey there');
    const first = h.replies[0].row.created;
    expect(h.replies[1].row.created).toBe(first);

    h.call.value = speaking(' On it', 2);
    expect(h.replies[2].row.created).toBe(first);
  });

  /** The engine wrote the reply down while the bridge still held the words.
   *  Redrawing would paint the whole reply twice, once under the row that
   *  landed. So the next delta opens a bubble carrying its own stretch only. */
  it('opens a new bubble once the engine claims the one standing', () => {
    let landed = false;
    const h = bridged(() => landed);
    h.call.value = speaking('One: only you from the UI.');
    landed = true;
    h.call.value = speaking(' Two: also a CLI verb.', 2);

    const last = h.replies[h.replies.length - 1];
    expect(last.row.text).toBe(' Two: also a CLI verb.');
    expect(last.fresh).toBe(true);
    expect(last.row.eventId).not.toBe(h.replies[0].row.eventId);
  });

  /** The engine's row arrives on SSE while the caller is still hearing the
   *  tail of the reply. Withdrawing on the turn's end would blank it. */
  it('stands until the engine writes the reply down', () => {
    const thread = withADoerWorking();
    thread.liveReply = { eventId: 'live-reply:t:1', created: '2026-08-31T07:16:02Z', text: 'On it' };
    const map = new Map([[THREAD, thread]]);

    handleEvent(map, THREAD, 9, {
      type: 'SpokenReplyGenerated', session_id: 'sess-1', text: 'On it', interrupted: false,
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');

    expect(map.get(THREAD)?.liveReply).toBeUndefined();
  });

  /** The backstop, for a reply the engine writes no row for. */
  it('goes when the call rings off', () => {
    const thread = withADoerWorking();
    thread.liveReply = { eventId: 'live-reply:t:1', created: '2026-08-31T07:16:02Z', text: 'Bye' };
    const map = new Map([[THREAD, thread]]);

    handleEvent(map, THREAD, 9, {
      type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 12,
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');

    expect(map.get(THREAD)?.liveReply).toBeUndefined();
  });

  /** **The wandering-header regression.** The row used to be appended as an
   *  exchange of its own. So it drew a second Lucidos Agent header under the
   *  first, and that header came and went as each persisted row landed.
   *
   *  It goes INSIDE the block its own persisted row will be filed into, which
   *  is what makes the swap move nothing on screen. */
  it('draws inside the running block, opening no boundary of its own', () => {
    const thread = withADoerWorking();
    const before = computeExchanges(thread).length;
    thread.liveReply = { eventId: 'live-reply:t:1', created: '2026-08-31T07:16:02Z', text: 'On it' };

    const rows = computeExchanges(thread);
    expect(rows).toHaveLength(before);
    const last = rows[rows.length - 1];
    expect(isLiveReplyRow(last.userEvent)).toBe(false);

    const step = last.steps[last.steps.length - 1];
    expect(isLiveReplyRow(step.event)).toBe(true);
    expect(step.event.type).toBe('SpokenReplyGenerated');
  });

  /** The talker spoke before anybody else did, so there is no block to go in.
   *  It opens one, exactly as a persisted greeting does. */
  it('opens a boundary when nothing can hold it', () => {
    const thread = makeOptimisticThreadState({
      id: THREAD, title: 'A call', channel: 'chat', initiator: 'user', eventsLoaded: true,
    });
    thread.liveReply = { eventId: 'live-reply:t:1', created: '2026-08-31T07:16:02Z', text: 'Hi!' };

    const rows = computeExchanges(thread);
    expect(rows).toHaveLength(1);
    expect(isLiveReplyRow(rows[0].userEvent)).toBe(true);
  });

  /** The call ends while the talker is mid-reply. What it had said is the
   *  caller's account of what they heard, and `call.rs` writes that down for
   *  every end reason. So the sweep is what finally retires the row. */
  it('keeps a reply in flight until the call itself ends', () => {
    installLiveUtteranceRow();
    const thread = withADoerWorking();
    threadMap.value = new Map([[THREAD, thread]]);

    voiceCall.value = { ...CALL_IDLE, phase: 'speaking', threadId: THREAD, said: 'On i', replyCount: 1 };
    expect(thread.liveReply?.text).toBe('On i');

    // The caller rings off mid-word. The row stands: the engine still owes one.
    voiceCall.value = CALL_IDLE;
    expect(thread.liveReply?.text).toBe('On i');

    handleEvent(threadMap.value, THREAD, 9, {
      type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'hangup', duration_secs: 12,
    } as StoredEvent, '2026-08-31T07:16:09Z', 'e-9');
    expect(thread.liveReply).toBeUndefined();
  });

  /** One clock stamps both sides, so a caller cutting in reads BELOW the reply
   *  they cut into. Sorted rather than blocked: the caller's rows used to take
   *  the whole synthetic range on their own. */
  it('reads in the order the two sides spoke', () => {
    const thread = withADoerWorking();
    thread.liveReply = { eventId: 'live-reply:t:1', created: '2026-08-31T07:16:02Z', text: 'On it' };
    thread.liveUtterances = [
      { eventId: liveUtteranceId(THREAD, 1), count: 1, created: '2026-08-31T07:16:01Z', text: 'first' },
      { eventId: liveUtteranceId(THREAD, 2), count: 2, created: '2026-08-31T07:16:03Z' },
    ];

    const tail = computeExchanges(thread).slice(-2);
    expect(tail.map(e => e.userEvent._eventId)).toEqual([
      liveUtteranceId(THREAD, 1),
      liveUtteranceId(THREAD, 2),
    ]);
    // The reply reads between them, as a step of the bubble it answered.
    expect(tail[0].steps.map(s => s.event._eventId)).toEqual(['live-reply:t:1']);
    expect(tail[1].steps).toEqual([]);
    // A `userSeq` is an identity, so the merged block must not reuse one.
    expect(new Set(tail.map(e => e.userSeq)).size).toBe(2);
  });
});

/**
 * The caller's own words as the provider hears them.
 *
 * A partial captions the bubble and settles nothing. `heard` is what a
 * persisted row claims, so a sentence still being said can never be retired by
 * one written for the sentence before it.
 */
describe('a partial captions the bubble and claims nothing', () => {
  const partialRow: LiveUtterance = {
    eventId: liveUtteranceId(THREAD, 1),
    count: 1,
    created: '2026-08-31T07:16:01Z',
    partial: 'the transcript is',
  };

  it('draws the words instead of the pulse', () => {
    const thread = withADoerWorking();
    thread.liveUtterances = [partialRow];
    const rows = computeExchanges(thread);
    const row = rows[rows.length - 1];

    expect(isLiveUtteranceRow(row.userEvent)).toBe(true);
    expect((row.userEvent as { text: string }).text).toBe('the transcript is');
  });

  it('survives a persisted row landing for an earlier sentence', () => {
    const thread = withADoerWorking();
    thread.liveUtterances = [partialRow];
    const map = new Map([[THREAD, thread]]);

    handleEvent(map, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: 'something else',
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');

    expect(map.get(THREAD)?.liveUtterances).toEqual([partialRow]);
  });
});

/**
 * **The orphan-bubble regression.** `call.rs` accumulates a caller stretch
 * across provider items, so a row drawn part-way through holds a PREFIX of
 * what finally lands. Matched on equality alone it is an orphan.
 *
 * What the reader saw was a bubble reading `bit` under a "Requesting" header,
 * standing until the hangup swept it away.
 */
describe('a landing row retires the rows it grew out of', () => {
  const stretch = 'Let\'s try this for a bit';

  it('takes the earlier rows of its own stretch', () => {
    const thread = withADoerWorking();
    thread.liveUtterances = [
      { eventId: liveUtteranceId(THREAD, 1), count: 1, created: '2026-08-31T07:16:01Z', text: 'Let\'s try this for a' },
      { eventId: liveUtteranceId(THREAD, 2), count: 2, created: '2026-08-31T07:16:02Z', text: stretch },
    ];
    const map = new Map([[THREAD, thread]]);

    handleEvent(map, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: stretch,
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');

    expect(map.get(THREAD)?.liveUtterances).toEqual([]);
    expect(map.get(THREAD)?.unclaimedUtterances).toEqual([]);
  });

  /** Prefix, never containment. Those words appear in the middle of the
   *  landing row, and the caller is still saying that sentence. */
  it('leaves a row whose words merely appear inside it', () => {
    const inFlight: LiveUtterance = {
      eventId: liveUtteranceId(THREAD, 1),
      count: 1,
      created: '2026-08-31T07:16:01Z',
      text: 'try this',
    };
    const thread = withADoerWorking();
    thread.liveUtterances = [inFlight];
    const map = new Map([[THREAD, thread]]);

    handleEvent(map, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: stretch,
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');

    expect(map.get(THREAD)?.liveUtterances).toEqual([inFlight]);
  });

  /** A row with no words yet is not a prefix of anything. That is the
   *  barge-in guarantee, restated against the new arm. */
  it('leaves a row that has said nothing yet', () => {
    const pulsing: LiveUtterance = {
      eventId: liveUtteranceId(THREAD, 1),
      count: 1,
      created: '2026-08-31T07:16:01Z',
    };
    const thread = withADoerWorking();
    thread.liveUtterances = [pulsing];
    const map = new Map([[THREAD, thread]]);

    handleEvent(map, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: stretch,
    } as StoredEvent, '2026-08-31T07:16:04Z', 'e-9');

    expect(map.get(THREAD)?.liveUtterances).toEqual([pulsing]);
  });
});
