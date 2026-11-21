// @vitest-environment jsdom
/** A turn's header names the backend of the session that turn ran in. A
 *  handoff may switch backend (ADR 0560), and the thread's own backend is its
 *  newest session's. Reading that one relabelled every earlier turn, so a
 *  Claude Code to Codex handoff left no record of the switch.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { renderExchanges } from '../CreateThreadView';
import { cancelingThreadIds, removingQueuedMessageIds, threadMap } from '../../../store/store';
import type { CodingAgent } from '../../../api/types';
import type { SessionContinuity } from '../../../generated/thread-event-wire';
import type { Exchange, StoredEvent, ThreadState } from '../../../store/thread-events';
import { step } from '../../../store/__tests__/fixtures';

const TS = '2026-10-23T12:00:00Z';

function makeThread(codingAgent: CodingAgent): ThreadState {
  return {
    meta: {
      id: 't1',
      title: 'Handoff test',
      channel: 'claude_code',
      initiator: 'user',
      saved: false,
      createdAt: TS,
      updatedAt: TS,
      status: 'idle',
      summaryVersion: 0,
      messageCount: 0,
      section: 'inbox',
      activeChildrenCount: 0,
      totalChildrenCount: 0,
      blockingDescendantCount: 0,
      attentionDescendantCount: 0,
      codingAgentChangeState: { kind: 'none' },
      codingAgentIsExternalRepo: false,
      lastRevivedAt: '',
      state: 'active',
      latestTodoList: null,
      liveEventWaitCount: 0,
      liveEventWaits: [],
      codingAgent,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: true,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function sessionStarted(seq: number, codingAgent: CodingAgent, continuity: SessionContinuity) {
  return step(seq, { type: 'SessionStarted', session_id: '', coding_agent: codingAgent, continuity, created: TS, _eventId: `s-${seq}` } as StoredEvent);
}

const HANDOFF: SessionContinuity = { kind: 'handoff', opening: 'narrative', message: 'Carry on.' } as SessionContinuity;

/** A finished turn, opening a session first when `session` is given. */
function turn(seq: number, session?: ReturnType<typeof sessionStarted>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: `ask ${seq}`, created: TS, channel: 'claude_code', _eventId: `u-${seq}` } as StoredEvent,
    userSeq: seq,
    steps: [
      ...(session ? [session] : []),
      step(seq + 0.2, { type: 'TextStreamed', text: `reply ${seq}`, created: TS, _eventId: `t-${seq}` } as StoredEvent),
      step(seq + 0.3, { type: 'ResponseGenerated', text: `reply ${seq}`, created: TS, _eventId: `r-${seq}` } as StoredEvent),
    ],
  };
}

function question(seq: number): Exchange {
  return {
    userEvent: { type: 'UserQuestionAsked', tool_use_id: `q-${seq}`, question: 'Which way?', options: [{ label: 'Left' }, { label: 'Right' }], created: TS, _eventId: `q-${seq}` } as StoredEvent,
    userSeq: seq,
    steps: [step(seq + 0.5, { type: 'UserQuestionAnswered', tool_use_id: `q-${seq}`, answer: { kind: 'Selected', option_id: 'Left' }, created: TS, _eventId: `a-${seq}` } as StoredEvent)],
  };
}

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  cancelingThreadIds.value = new Set();
  removingQueuedMessageIds.value = new Set();
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

function draw(exchanges: Exchange[], threadBackend: CodingAgent): void {
  threadMap.value = new Map([['t1', makeThread(threadBackend)]]);
  act(() => { render(<div>{renderExchanges(exchanges, 't1', '')}</div>, host); });
}

const headers = () => [...host.querySelectorAll('.response-executor-label')].map(el => el.textContent);
const askers = () => [...host.querySelectorAll('.initiator-label')].map(el => el.textContent);

describe('a turn header names its own session backend', () => {
  it('reads Claude before a Claude Code to Codex handoff, and Codex after it', () => {
    draw([
      turn(1, sessionStarted(1.1, 'claude-code', { kind: 'first' })),
      question(2),
      turn(3),
      turn(4, sessionStarted(4.1, 'codex', HANDOFF)),
      turn(5),
    ], 'codex');
    expect(headers()).toEqual(['Claude', 'Claude', 'Codex', 'Codex']);
    expect(askers()).toEqual(['Claude']);
  });

  it('names each handoff card by the session it opened', () => {
    draw([
      turn(1, sessionStarted(1.1, 'claude-code', { kind: 'first' })),
      turn(2, sessionStarted(2.1, 'claude-code', HANDOFF)),
      turn(3, sessionStarted(3.1, 'codex', HANDOFF)),
    ], 'codex');
    expect(headers()).toEqual(['Claude', 'Claude', 'Codex']);
  });

  // An older page that is not loaded yet holds the first session. A resume or
  // a fresh session keeps the backend, so the turns above it share its own.
  it('carries a resumed session backend up to the turns above it', () => {
    draw([turn(1), turn(2, sessionStarted(2.1, 'codex', { kind: 'resumed' }))], 'codex');
    expect(headers()).toEqual(['Codex', 'Codex']);
  });

  // A handoff can switch backend, so nothing loaded says what ran above it.
  it('falls back to the thread backend above a handoff whose session is not loaded', () => {
    draw([turn(1), turn(2, sessionStarted(2.1, 'claude-code', HANDOFF))], 'codex');
    expect(headers()).toEqual(['Codex', 'Claude']);
  });

  it('reads a legacy session with no backend as Claude Code', () => {
    const legacy = step(1.1, { type: 'SessionStarted', session_id: '', created: TS, _eventId: 's-legacy' } as StoredEvent);
    draw([turn(1, legacy), turn(2, sessionStarted(2.1, 'codex', HANDOFF))], 'codex');
    expect(headers()).toEqual(['Claude', 'Codex']);
  });
});
