import { describe, it, expect } from 'vitest';
import { exchangeResponseEvents, exchangeStatus, exchangeSteps, type AnswerKind, type Exchange, type ThreadEvent, type ThreadMeta, type ThreadState } from '../thread-events';
import type { StoredEvent } from '../thread-events';
import {
  findLatestPendingQuestion,
  computeSubmitMultiCount,
  promptPlaceholder,
  PLACEHOLDER_ANSWERING,
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
    codingAgentProposed: false,
    codingAgentRequiresRestart: false,
    codingAgentIsExternalRepo: false,
    codingAgentApplying: false,
    codingAgentHasDiff: false,
    lastRevivedAt: '',
    messageCount: 1,
    section: 'archived',
    activeChildrenCount: 0,
    totalChildrenCount: 0,
    blockingDescendantCount: 0, attentionDescendantCount: 0,
    state: 'active',
    latestTodoList: null,
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
    // After UserQuestionAnswered, the engine emits a CodingAgentPromptSent
    // resume marker (empty text → Thinking spinner). Then CC's PreToolUse
    // hook unblocks and CC processes the synthetic tool_result for the
    // AskUserQuestion, which surfaces as CodingAgentToolResult. That
    // CodingAgentToolResult must NOT resolve the resume-marker Thinking
    // spinner — the spinner should keep spinning until CC produces real
    // output (text or a non-AskUserQuestion tool call).
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

    // The resume-marker Thinking step must still be a spinner (success: null).
    const events = exchangeResponseEvents(ex, true);
    const stepEvents = events.filter(e => e.type === 'step') as Array<{ description: string; outcome: StepOutcome }>;
    const trailingThinking = stepEvents[stepEvents.length - 1];
    expect(trailingThinking.description).toBe('Thinking');
    expect(trailingThinking.outcome).toBe('pending');

    // exchangeSteps (the parallel projection used by other UI surfaces) must agree.
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
    // Mirrors the Rust `text: Option<String>` field. The prompt-row Submit
    // bundles the textarea contents into the answer when a multi-select
    // question is pending.
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

// PromptInput derives BOTH of its per-render question facts from this one
// walk: `multiSelect` picks the prompt-row Submit control, and the mere
// presence of a pending question switches the placeholder to the answering
// one. A second walk per keystroke is what the helper's own gating comment
// exists to avoid, so there is no separate multi-select finder to test.
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
    // Single-select answers through the card, so no prompt-row Submit. The
    // placeholder still flips: typing is a valid freetext answer to it.
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
    expect(promptPlaceholder(true, true)).toBe(PLACEHOLDER_ANSWERING);
  });

  // The card renders the question and its options, nothing else, so the two
  // escapes that need no option slot are named by the prompt row. This
  // placeholder is the typing half: text typed here routes to the pending
  // question as a `FreeText` answer. Cancel (which stamps it `Canceled`) is
  // named by the Cancel button's tooltip instead, `ANSWER_CANCEL_TOOLTIP`,
  // pinned in `components/chat/__tests__/question-card.test.tsx`. Drop either
  // half and an agent starts inventing an "Other, I'll type it" option, which
  // just hands its label back as the answer.
  it('names the typing escape, since nothing on the card does', () => {
    expect(PLACEHOLDER_ANSWERING).toBe('Type custom answer here…');
  });

  // A permission card leaves the thread `waiting_for_user_answer` with no
  // pending question, and typed text there is an ordinary message.
  it('keeps the follow-up placeholder when no question card is pending', () => {
    expect(promptPlaceholder(true, false)).toBe(PLACEHOLDER_FOLLOW_UP);
  });

  it('keeps the compose placeholder with no focused thread', () => {
    expect(promptPlaceholder(false, false)).toBe(PLACEHOLDER_NEW_THREAD);
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
