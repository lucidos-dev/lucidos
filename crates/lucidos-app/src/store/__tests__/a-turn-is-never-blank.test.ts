import { afterEach, describe, expect, it } from 'vitest';
import { put } from './call-fixtures';
import { installLiveUtteranceRow } from '../liveUtterance';
import { _applyEventRowsForTest } from '../actions/thread-loading';
import { threadMap } from '../store';
import { voiceCall } from '../voiceCall';
import {
  computeExchanges,
  exchangeUserMessage,
  handleEvent,
  isLiveUtteranceRow,
  makeOptimisticThreadState,
  type StoredEvent,
  type ThreadState,
} from '../thread-events';
import { CALL_IDLE, type CallState } from '../../voice/callState';

const THREAD = 'thread-1';
const WORDS = 'fix the blank thread';

function aCalledThread(): ThreadState {
  const thread = makeOptimisticThreadState({
    id: THREAD,
    title: 'A call',
    channel: 'chat',
    initiator: 'user',
    eventsLoaded: true,
    status: 'idle',
  });
  put(thread.events, 1, { type: 'VoiceSessionStarted', session_id: 'sess-1' });
  return thread;
}

function onACall() {
  installLiveUtteranceRow();
  const thread = aCalledThread();
  threadMap.value = new Map([[THREAD, thread]]);
  const live = (over: Partial<CallState>): void => {
    voiceCall.value = {
      ...CALL_IDLE,
      phase: 'listening',
      threadId: THREAD,
      utterance: 'live',
      utteranceCount: 1,
      ...over,
    };
  };
  return { thread, live };
}

function lastRow(): 'nothing' | 'speaking' | 'words' {
  const exchanges = computeExchanges(threadMap.value.get(THREAD)!);
  const last = exchanges[exchanges.length - 1];
  if (!last) return 'nothing';
  if (!isLiveUtteranceRow(last.userEvent)) return 'words';
  return exchangeUserMessage(last) ? 'words' : 'speaking';
}

afterEach(() => {
  voiceCall.value = CALL_IDLE;
});

describe('the words land in the frame the speaking stops', () => {
  it('shows the caller\'s words with no event landing in between', () => {
    const { live } = onACall();
    live({});
    expect(lastRow()).toBe('speaking');

    live({ utterance: 'transcribed', heard: WORDS });

    const exchanges = computeExchanges(threadMap.value.get(THREAD)!);
    expect(exchangeUserMessage(exchanges[exchanges.length - 1])).toBe(WORDS);
  });

  it('never passes through an empty transcript', () => {
    const { live } = onACall();
    const seen: string[] = [];
    seen.push(lastRow());
    live({});
    seen.push(lastRow());
    live({ utterance: 'landing' });
    seen.push(lastRow());
    live({ utterance: 'transcribed', heard: WORDS });
    seen.push(lastRow());
    live({ utterance: 'none' });
    seen.push(lastRow());
    voiceCall.value = CALL_IDLE;
    seen.push(lastRow());

    expect(seen).toEqual(['nothing', 'speaking', 'speaking', 'words', 'words', 'words']);
  });
});

describe('the row stands until the engine writes the words down', () => {
  it('survives the bound on waiting for the words', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    live({ utterance: 'none' });
    expect(thread.liveUtterances?.[0].text).toBe(WORDS);
  });

  it('survives the hangup, which the engine writes the words down through', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    voiceCall.value = { ...voiceCall.value, phase: 'ending', utterance: 'none', heard: null };
    expect(thread.liveUtterances?.[0].text).toBe(WORDS);
  });

  it('withdraws a row that never got any words', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'none' });
    expect(thread.liveUtterances).toEqual([]);
  });
});

describe('the engine\'s own row replaces it, and never joins it', () => {
  it('leaves exactly one bubble when the talker answered alone', () => {
    const { live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    handleEvent(threadMap.value, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: WORDS,
    } as StoredEvent, '2026-09-05T20:58:23Z', 'e-9');

    const spoken = computeExchanges(threadMap.value.get(THREAD)!)
      .filter(ex => exchangeUserMessage(ex) === WORDS);
    expect(spoken).toHaveLength(1);
    expect(isLiveUtteranceRow(spoken[0].userEvent)).toBe(false);
  });

  it('leaves exactly one bubble when the talker delegated it', () => {
    const { live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    handleEvent(threadMap.value, THREAD, 9, {
      type: 'MessageReceived', text: WORDS, mode: 'human', channel: 'chat',
      voice_session_id: 'sess-1',
    } as StoredEvent, '2026-09-05T20:58:23Z', 'e-9');

    const spoken = computeExchanges(threadMap.value.get(THREAD)!)
      .filter(ex => exchangeUserMessage(ex) === WORDS);
    expect(spoken).toHaveLength(1);
  });
});

describe('the call ending sweeps whatever the engine wrote no row for', () => {
  it('drops a row the engine never wrote down', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    expect(thread.liveUtterances).toHaveLength(1);

    handleEvent(threadMap.value, THREAD, 9, {
      type: 'VoiceSessionEnded', session_id: 'sess-1', reason: 'agent_hangup', duration_secs: 30,
    } as StoredEvent, '2026-09-05T20:58:37Z', 'e-9');

    expect(thread.liveUtterances).toEqual([]);
  });

  it('is not what clears a row whose words landed', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    handleEvent(threadMap.value, THREAD, 9, {
      type: 'SpokenMessageReceived', session_id: 'sess-1', text: WORDS,
    } as StoredEvent, '2026-09-05T20:58:30Z', 'e-9');
    expect(thread.liveUtterances).toEqual([]);
    expect(thread.unclaimedUtterances ?? []).toEqual([]);
  });
});

describe('loading the history leaves a live bubble alone', () => {
  it('keeps the row a past call\'s events would have taken', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });

    _applyEventRowsForTest(threadMap.value, thread, [
      { sequence: 100, event_type: 'VoiceSessionStarted', payload: { session_id: 'old' }, created: '2026-09-01T10:00:00Z', event_id: 'h-1' },
      { sequence: 101, event_type: 'SpokenMessageReceived', payload: { session_id: 'old', text: 'last week' }, created: '2026-09-01T10:00:05Z', event_id: 'h-2' },
      { sequence: 102, event_type: 'VoiceSessionEnded', payload: { session_id: 'old', reason: 'agent_hangup', duration_secs: 20 }, created: '2026-09-01T10:00:20Z', event_id: 'h-3' },
    ]);

    expect(thread.liveUtterances?.[0].text).toBe(WORDS);
    expect(thread.unclaimedUtterances ?? []).toEqual([]);
  });

  it('keeps a reply a past call\'s session end would have swept', () => {
    const { thread } = onACall();
    thread.liveReply = { eventId: 'live-reply:t:1', created: '2026-09-05T20:58:00Z', text: 'On it' };

    _applyEventRowsForTest(threadMap.value, thread, [
      { sequence: 100, event_type: 'VoiceSessionEnded', payload: { session_id: 'old', reason: 'hangup', duration_secs: 20 }, created: '2026-09-01T10:00:20Z', event_id: 'h-1' },
    ]);

    expect(thread.liveReply?.text).toBe('On it');
  });

  it('claims a standing row when the current call\'s words arrive in a replay', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    expect(thread.liveUtterances?.[0].text).toBe(WORDS);

    _applyEventRowsForTest(threadMap.value, thread, [
      { sequence: 200, event_type: 'SpokenMessageReceived', payload: { session_id: 'sess-1', text: WORDS }, created: '2026-09-05T20:58:31Z', event_id: 'h-9' },
    ]);

    expect(thread.liveUtterances).toEqual([]);
  });
});

describe('a revision rewrites the row', () => {
  it('replaces the text rather than adding a second bubble', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: 'fix the blank thread' });
    const first = thread.liveUtterances?.[0].created;
    live({ utterance: 'transcribed', heard: 'fix the blank thread view' });

    expect(thread.liveUtterances).toHaveLength(1);
    expect(thread.liveUtterances?.[0].text).toBe('fix the blank thread view');
    expect(thread.liveUtterances?.[0].created).toBe(first);
  });
});

describe('a caller who carries on speaking', () => {
  it('keeps the first sentence on screen while saying the next', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    live({ utteranceCount: 2 });

    expect(thread.liveUtterances?.map(r => r.text)).toEqual([WORDS, undefined]);
    const exchanges = computeExchanges(thread);
    expect(exchangeUserMessage(exchanges[exchanges.length - 2])).toBe(WORDS);
  });

  it('retires a leftover row when a second call reuses its count', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    voiceCall.value = CALL_IDLE;
    expect(thread.liveUtterances?.[0].text).toBe(WORDS);

    live({});
    expect(thread.liveUtterances?.map(r => r.text)).toEqual([undefined]);
    expect(thread.unclaimedUtterances ?? []).toEqual([]);
  });

  it('never shares a seq with a message typed during the call', () => {
    const { thread, live } = onACall();
    thread.pendingUserMessages.push({
      text: 'typed while talking', eventId: 'p-1', created: '2026-09-05T20:58:00Z',
    });
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    live({ utteranceCount: 2 });

    const seqs = computeExchanges(thread).map(ex => ex.userSeq);
    expect(new Set(seqs).size).toBe(seqs.length);
  });

  it('retires them oldest first as the engine catches up', () => {
    const { thread, live } = onACall();
    live({});
    live({ utterance: 'transcribed', heard: WORDS });
    live({ utteranceCount: 2 });
    live({ utteranceCount: 2, utterance: 'transcribed', heard: 'and the tests' });

    const land = (seq: number, text: string): void => {
      handleEvent(threadMap.value, THREAD, seq, {
        type: 'SpokenMessageReceived', session_id: 'sess-1', text,
      } as StoredEvent, `2026-09-05T20:58:2${seq}Z`, `e-${seq}`);
    };
    land(9, WORDS);
    expect(thread.liveUtterances?.map(r => r.count)).toEqual([2]);
    land(10, 'and the tests');
    expect(thread.liveUtterances).toEqual([]);
  });
});
