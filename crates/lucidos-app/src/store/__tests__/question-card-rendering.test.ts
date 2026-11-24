import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, exchangeSteps, type AnswerKind, type Exchange, type ThreadEvent, type ThreadMeta, type ThreadState } from '../thread-events';
import type { StoredEvent } from '../thread-events';
import {
  findLatestPendingQuestion,
  computeSubmitMultiCount,
  promptPlaceholder,
  threadHasConversation,
  PLACEHOLDER_AGENT_PAUSED,
  PLACEHOLDER_ANSWERING,
  PLACEHOLDER_SIDE_QUESTION,
  PLACEHOLDER_FOLLOW_UP,
  PLACEHOLDER_NEW_THREAD,
} from '../../components/chat/PromptInput';
import type { StepOutcome } from '../types';

function step(seq: number, event: Partial<StoredEvent> & { type: string }): { seq: number; event: StoredEvent } {
  return { seq, event: event as StoredEvent };
}

function exchange(steps: Array<{ seq: number; event: StoredEvent }>): Exchange {
  return {
    userEvent: { type: 'MessageReceived', text: 'pls help' } as StoredEvent,
    userSeq: 0,
    steps,
  };
}

function buildThreadState(events: ThreadEvent[]): ThreadState {
  const map = new Map<number, ThreadEvent>();
  events.forEach((ev, i) => map.set(i + 1, ev));
  const meta: ThreadMeta = {
    id: 'thread-1',
    title: 'Test',
    channel: 'claude_code',
    initiator: 'user',
    saved: false,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    status: 'waiting_for_user_answer',
    summaryVersion: 0,
    codingAgentChangeState: { kind: 'none' },
    codingAgentIsExternalRepo: false,
    lastRevivedAt: '',
    messageCount: 1,
    section: 'archived',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0, attentionDescendantCount: 0,
    state: 'active',
    latestTodoList: null,
    liveEventWaitCount: 0,
    liveEventWaits: [],
  };
  return { meta, events: map, streamingBuffer: '', eventsLoaded: true, eventsLoadFailed: false, lastDbSeq: events.length, pendingUserMessages: [] };
}

describe('exchangeStatus + spinner behavior around UserQuestionAsked', () => {
  it('exchangeStatus reads as awaiting-answer while waiting for an answer (no spinner, no Done label)', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'thinking…' }),
      step(3, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_3',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [{ id: 'opt-0', label: 'A' }],
      }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('awaiting-answer');
  });

  it('exchangeStatus returns to coding-agent-working once CC resumes after answer', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_4',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [],
      }),
      step(3, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_4',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      }),
      step(4, { type: 'CodingAgentTextStreamed', text: 'continuing…' }),
    ]);
    expect(exchangeStatus(ex, '', true, false, true)).toBe('coding-agent-working');
  });

  it('keeps resume-marker Thinking step spinning when AskUserQuestion ToolResult fires', () => {
    const ex = exchange([
      step(1, { type: 'SessionStarted', session_id: 'sess', branch: '' }),
      step(2, { type: 'CodingAgentTextStreamed', text: 'thinking…' }),
      step(3, {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_resume',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [{ id: 'opt-0', label: 'A' }],
      }),
      step(4, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_resume',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      }),
      step(5, { type: 'CodingAgentPromptSent', text: '' }),
      step(6, { type: 'CodingAgentToolResult', name: '', result: 'opt-0' }),
    ]);

    const events = exchangeResponseEvents(ex, true);
    const stepEvents = events.filter(e => e.type === 'step') as Array<{ description: string; outcome: StepOutcome }>;
    const trailingThinking = stepEvents[stepEvents.length - 1];
    expect(trailingThinking.description).toBe('Thinking');
    expect(trailingThinking.outcome).toBe('pending');

    const steps = exchangeSteps(ex, true, false);
    const lastStep = steps[steps.length - 1];
    expect(lastStep.description).toBe('Thinking');
    expect(lastStep.outcome).toBe('pending');
  });
});

describe('AnswerKind discriminated union', () => {
  it('includes MultiSelected with option_ids', () => {
    const a: AnswerKind = { kind: 'MultiSelected', option_ids: ['opt-0', 'opt-1'] };
    expect(a.kind).toBe('MultiSelected');
    if (a.kind === 'MultiSelected') {
      expect(a.option_ids).toHaveLength(2);
      expect(a.option_ids[0]).toBe('opt-0');
    }
  });

  it('MultiSelected accepts optional text alongside option_ids', () => {
    const a: AnswerKind = { kind: 'MultiSelected', option_ids: ['opt-0'], text: 'plus this' };
    if (a.kind === 'MultiSelected') {
      expect(a.text).toBe('plus this');
    }
    const b: AnswerKind = { kind: 'MultiSelected', option_ids: [], text: 'just text' };
    if (b.kind === 'MultiSelected') {
      expect(b.option_ids).toHaveLength(0);
      expect(b.text).toBe('just text');
    }
  });
});

describe('findLatestPendingQuestion', () => {
  it('returns the toolUseId of the latest unanswered multi-select question', () => {
    const thread = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'claude_code' } as ThreadEvent,
      { type: 'SessionStarted', session_id: 'sess', branch: '' } as ThreadEvent,
      {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_pending',
        cc_session_id: 'sess',
        question: 'Pick all that apply:',
        options: [{ id: 'opt-0', label: 'A' }, { id: 'opt-1', label: 'B' }],
        multi_select: true,
      } as ThreadEvent,
    ]);
    expect(findLatestPendingQuestion(thread)).toEqual({
      toolUseId: 'tu_pending',
      multiSelect: true,
    });
  });

  it('returns null when the multi-select question is already answered', () => {
    const thread = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'claude_code' } as ThreadEvent,
      { type: 'SessionStarted', session_id: 'sess', branch: '' } as ThreadEvent,
      {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_done',
        cc_session_id: 'sess',
        question: 'Pick:',
        options: [{ id: 'opt-0', label: 'A' }],
        multi_select: true,
      } as ThreadEvent,
      {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_done',
        answer: { kind: 'MultiSelected', option_ids: ['opt-0'] },
      } as ThreadEvent,
    ]);
    expect(findLatestPendingQuestion(thread)).toBeNull();
  });

  it('reports a single-select question as pending but not multi-select', () => {
    const thread = buildThreadState([
      { type: 'MessageReceived', text: 'go', channel: 'claude_code' } as ThreadEvent,
      { type: 'SessionStarted', session_id: 'sess', branch: '' } as ThreadEvent,
      {
        type: 'UserQuestionAsked',
        tool_use_id: 'tu_single',
        cc_session_id: 'sess',
        question: 'Pick one:',
        options: [{ id: 'opt-0', label: 'A' }],
        multi_select: false,
      } as ThreadEvent,
    ]);
    expect(findLatestPendingQuestion(thread)).toEqual({
      toolUseId: 'tu_single',
      multiSelect: false,
    });
  });

  it('returns null for an undefined thread (focused thread missing from map)', () => {
    expect(findLatestPendingQuestion(undefined)).toBeNull();
  });
});

describe('promptPlaceholder', () => {
  it('invites an answer while a question card is pending', () => {
    expect(promptPlaceholder(true, true, false)).toBe(PLACEHOLDER_ANSWERING);
  });

  it('names the typing escape, since nothing on the card does', () => {
    expect(PLACEHOLDER_ANSWERING).toBe('Type custom answer here…');
  });

  it('keeps the follow-up placeholder when no question card is pending', () => {
    expect(promptPlaceholder(true, false, false)).toBe(PLACEHOLDER_FOLLOW_UP);
  });

  it('keeps the compose placeholder with no focused thread', () => {
    expect(promptPlaceholder(false, false, false)).toBe(PLACEHOLDER_NEW_THREAD);
  });

  it('invites a side question in side-question mode', () => {
    expect(promptPlaceholder(true, false, true)).toBe(PLACEHOLDER_SIDE_QUESTION);
  });

  it('invites a side question over a pending card in side-question mode', () => {
    expect(promptPlaceholder(true, true, true)).toBe(PLACEHOLDER_SIDE_QUESTION);
  });

  it('says the agent is paused, so nothing else has to (ADR 0657)', () => {
    expect(promptPlaceholder(true, false, false, true)).toBe(PLACEHOLDER_AGENT_PAUSED);
  });

  it('lets a pending card and the side-question mode win over the pause', () => {
    expect(promptPlaceholder(true, true, false, true)).toBe(PLACEHOLDER_ANSWERING);
    expect(promptPlaceholder(true, false, true, true)).toBe(PLACEHOLDER_SIDE_QUESTION);
  });
});

describe('threadHasConversation', () => {
  const message = { type: 'MessageReceived', text: 'hi' } as ThreadEvent;

  it('is false with no thread open', () => {
    expect(threadHasConversation(null, undefined)).toBe(false);
  });

  it('is true for a focused thread not yet in the thread list', () => {
    expect(threadHasConversation('thread-1', undefined)).toBe(true);
  });

  it('is true for a thread with a message', () => {
    expect(threadHasConversation('thread-1', buildThreadState([message]))).toBe(true);
  });

  it('is false for the home thread, even with messages', () => {
    const home = buildThreadState([message]);
    home.meta.home = true;
    expect(threadHasConversation('thread-1', home)).toBe(false);
    expect(threadHasConversation('thread-1', { ...home, events: new Map() })).toBe(false);
  });

  it('is false for a loaded thread with no messages', () => {
    expect(threadHasConversation('thread-1', buildThreadState([]))).toBe(false);
  });

  it('is false for a draft', () => {
    const draft = buildThreadState([]);
    draft.meta.state = 'composing';
    expect(threadHasConversation('thread-1', draft)).toBe(false);
  });

  it('is true for a thread whose first message is still in flight', () => {
    const sending = buildThreadState([]);
    sending.pendingUserMessages = [{ text: 'hi' } as ThreadState['pendingUserMessages'][number]];
    expect(threadHasConversation('thread-1', sending)).toBe(true);
  });

  it('is true for a thread still loading', () => {
    expect(threadHasConversation('thread-1', { ...buildThreadState([]), eventsLoaded: false })).toBe(true);
  });
});

describe('computeSubmitMultiCount', () => {
  it('returns 0 when no toggles and no text — Submit reads bare', () => {
    expect(computeSubmitMultiCount(0, '')).toBe(0);
  });

  it('counts toggled options', () => {
    expect(computeSubmitMultiCount(2, '')).toBe(2);
  });

  it('counts a typed custom answer as +1 when no options are toggled', () => {
    expect(computeSubmitMultiCount(0, 'something')).toBe(1);
  });

  it('counts toggled options + the typed custom answer', () => {
    expect(computeSubmitMultiCount(2, 'something')).toBe(3);
  });

  it('treats whitespace-only text as empty so the count matches what submit will send', () => {
    expect(computeSubmitMultiCount(0, '   \n\t')).toBe(0);
    expect(computeSubmitMultiCount(2, '   ')).toBe(2);
  });
});
