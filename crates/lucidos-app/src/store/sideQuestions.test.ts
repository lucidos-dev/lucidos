import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const post = vi.fn<(threadId: string, id: string, question: string, images: readonly string[]) => Promise<string>>();
const postDismissal = vi.fn<(threadId: string, id: string) => Promise<void>>();
const toast = vi.fn();
vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>();
  return {
    ...actual,
    askSideQuestion: (t: string, id: string, q: string, images: readonly string[]) => post(t, id, q, images),
    dismissSideQuestion: (t: string, id: string) => postDismissal(t, id),
  };
});
vi.mock('./store', async (importOriginal) => {
  const actual = await importOriginal<typeof import('./store')>();
  return { ...actual, showToast: (...args: unknown[]) => toast(...args) };
});

import { ApiError } from '../api/client';
import {
  SIDE_QUESTION_CODEX,
  SIDE_QUESTION_EMPTY,
  SIDE_QUESTION_NOT_STARTED,
  askSideQuestion,
  dismissSideQuestion,
  latestEventSeq,
  routeSideQuestion,
  localSideQuestions,
  dismissingSideQuestions,
  reopenSideQuestion,
  reopenedSideQuestions,
  sideQuestionModeOn,
  sideQuestionsFor,
} from './sideQuestions';
import { composeSelections } from './composeSelections';
import { threadMap } from './store';
import type { StoredEvent } from './thread-events/thread-event-types';
import type { ThreadState } from './thread-events/thread-meta';

const STARTED = { started: true, codex: false };

describe('sideQuestionModeOn', () => {
  afterEach(() => { composeSelections.value = new Map(); });

  it('reads the flag from the draft\'s compose selection', () => {
    composeSelections.value = new Map([['t1', { sideQuestionMode: true }], ['t2', { sideQuestionMode: false }]]);
    expect(sideQuestionModeOn('t1')).toBe(true);
    expect(sideQuestionModeOn('t2')).toBe(false);
    expect(sideQuestionModeOn('t3')).toBe(false);
  });

  it('is off with no thread', () => {
    composeSelections.value = new Map([['t1', { sideQuestionMode: true }]]);
    expect(sideQuestionModeOn(null)).toBe(false);
  });
});

describe('routeSideQuestion', () => {
  it('asks the trimmed draft when the user chose a side question', () => {
    expect(routeSideQuestion('  what is X?  ', STARTED, true)).toEqual({ kind: 'ask', question: 'what is X?' });
  });

  it('sends everything else as a message, /btw text included', () => {
    expect(routeSideQuestion('please fix the tests', STARTED, false)).toEqual({ kind: 'message' });
    expect(routeSideQuestion('/compact', STARTED, false)).toEqual({ kind: 'message' });
    expect(routeSideQuestion('/btw what is X?', STARTED, false)).toEqual({ kind: 'message' });
  });

  it('refuses, keeping the draft, where it cannot be asked', () => {
    expect(routeSideQuestion('q', { started: false, codex: false }, true))
      .toEqual({ kind: 'refuse', toast: SIDE_QUESTION_NOT_STARTED });
    expect(routeSideQuestion('   ', STARTED, true)).toEqual({ kind: 'refuse', toast: SIDE_QUESTION_EMPTY });
    // A Codex thread takes none, and refusing here keeps the draft's images.
    expect(routeSideQuestion('q', { started: true, codex: true }, true))
      .toEqual({ kind: 'refuse', toast: SIDE_QUESTION_CODEX });
  });
});

function resetSideQuestions() {
  localSideQuestions.value = new Map();
  dismissingSideQuestions.value = new Set();
  reopenedSideQuestions.value = new Set();
  threadMap.value = new Map();
  post.mockReset();
  postDismissal.mockReset();
  toast.mockReset();
}

/** Puts a thread holding `events` (seq to event) in the map. */
function holdEvents(threadId: string, events: [number, Record<string, unknown>][]) {
  const thread = { meta: { id: threadId }, events: new Map(events as [number, StoredEvent][]) };
  threadMap.value = new Map([...threadMap.value, [threadId, thread as unknown as ThreadState]]);
}

const asked = (id: string, question: string) => ({ type: 'SideQuestionAsked', side_question_id: id, question });

describe('askSideQuestion', () => {
  beforeEach(resetSideQuestions);
  afterEach(resetSideQuestions);

  it('shows a pending card, then the answer', async () => {
    let resolve!: (answer: string) => void;
    post.mockReturnValue(new Promise((r) => { resolve = r; }));
    const asking = askSideQuestion('t1', 'what is X?', ['h1']);
    const [card] = sideQuestionsFor('t1');
    expect(post).toHaveBeenCalledWith('t1', card.id, 'what is X?', ['h1']);
    expect(card).toMatchObject({
      question: 'what is X?', imageHashes: ['h1'], status: 'pending', dismissed: false,
    });
    resolve('X is a letter.');
    await asking;
    expect(sideQuestionsFor('t1')).toMatchObject([{ status: 'answered', answer: 'X is a letter.' }]);
  });

  it('gives every ask its own id', async () => {
    post.mockResolvedValue('a');
    await askSideQuestion('t1', 'one');
    await askSideQuestion('t1', 'two');
    const [a, b] = sideQuestionsFor('t1');
    expect(a.id).not.toBe(b.id);
  });

  it("puts the engine's refusal on the card, for a Codex thread", async () => {
    post.mockRejectedValue(new ApiError(400, 'Side questions are not available in Codex threads.'));
    await askSideQuestion('codex-thread', 'q');
    expect(sideQuestionsFor('codex-thread')).toMatchObject([
      { status: 'failed', error: 'Side questions are not available in Codex threads.' },
    ]);
  });

  it('records the newest event the thread held when asked', async () => {
    post.mockResolvedValue('a');
    holdEvents('t1', [[3, {}], [9, {}], [5, {}]]);
    await askSideQuestion('t1', 'q');
    expect(sideQuestionsFor('t1')).toMatchObject([{ afterSeq: 9, status: 'answered' }]);
  });

  it('records no event for a thread with none loaded', () => {
    expect(latestEventSeq(undefined)).toBeNull();
    expect(latestEventSeq({ events: new Map() } as unknown as ThreadState)).toBeNull();
  });

  it('stacks cards and keeps each thread to its own', async () => {
    post.mockResolvedValue('a');
    await askSideQuestion('t1', 'one');
    await askSideQuestion('t2', 'two');
    await askSideQuestion('t1', 'three');
    expect(sideQuestionsFor('t1').map((q) => q.question)).toEqual(['one', 'three']);
  });
});

describe('recorded side questions', () => {
  beforeEach(resetSideQuestions);
  afterEach(resetSideQuestions);

  it('draws each card from its events, at its asked seq', () => {
    holdEvents('t1', [
      [4, { ...asked('a', 'first'), image_hashes: ['h1'] }],
      [5, { type: 'MessageReceived' }],
      [6, asked('b', 'second')],
      [7, { type: 'SideQuestionAnswered', side_question_id: 'a', answer: 'one' }],
      [8, { type: 'SideQuestionFailed', side_question_id: 'b', error: 'Interrupted by a restart. Ask again.' }],
    ]);
    expect(sideQuestionsFor('t1')).toEqual([
      {
        id: 'a', threadId: 't1', question: 'first', imageHashes: ['h1'], afterSeq: 4, dismissed: false,
        status: 'answered', answer: 'one',
      },
      {
        id: 'b', threadId: 't1', question: 'second', imageHashes: [], afterSeq: 6, dismissed: false,
        status: 'failed', error: 'Interrupted by a restart. Ask again.',
      },
    ]);
  });

  it('shows a card still waiting as pending, and a dismissed one dismissed', () => {
    holdEvents('t1', [
      [1, asked('a', 'q')],
      [2, asked('b', 'r')],
      [3, { type: 'SideQuestionDismissed', side_question_id: 'b' }],
    ]);
    expect(sideQuestionsFor('t1')).toMatchObject([
      { id: 'a', status: 'pending', dismissed: false },
      { id: 'b', status: 'pending', dismissed: true },
    ]);
  });

  it('lets the recorded card replace the local one', async () => {
    post.mockResolvedValue('local answer');
    await askSideQuestion('t1', 'q');
    const id = sideQuestionsFor('t1')[0].id;
    holdEvents('t1', [[10, asked(id, 'q')], [11, { type: 'SideQuestionAnswered', side_question_id: id, answer: 'recorded' }]]);
    expect(sideQuestionsFor('t1')).toMatchObject([{ id, afterSeq: 10, status: 'answered', answer: 'recorded' }]);
  });

  it('shows the answer this device holds while the recorded card still waits', async () => {
    let resolve!: (answer: string) => void;
    post.mockReturnValue(new Promise((r) => { resolve = r; }));
    const asking = askSideQuestion('t1', 'q');
    const id = sideQuestionsFor('t1')[0].id;
    holdEvents('t1', [[10, asked(id, 'q')]]);
    resolve('here');
    await asking;
    const drawn = sideQuestionsFor('t1');
    expect(drawn).toMatchObject([{ id, afterSeq: 10, status: 'answered', answer: 'here' }]);
    expect(sideQuestionsFor('t1')[0]).toBe(drawn[0]);
  });

  it('keeps a card the same object while unrelated events arrive', () => {
    holdEvents('t1', [[1, asked('a', 'q')]]);
    const before = sideQuestionsFor('t1')[0];
    threadMap.value.get('t1')!.events.set(2, { type: 'MessageReceived' } as unknown as StoredEvent);
    expect(sideQuestionsFor('t1')[0]).toBe(before);
  });
});

describe('dismissSideQuestion', () => {
  beforeEach(resetSideQuestions);
  afterEach(resetSideQuestions);

  it('records the dismissal and draws the card dismissed at once', async () => {
    holdEvents('t1', [[1, asked('a', 'q')]]);
    let settle!: () => void;
    postDismissal.mockReturnValue(new Promise((r) => { settle = r; }));
    const dismissing = dismissSideQuestion(sideQuestionsFor('t1')[0]);
    expect(postDismissal).toHaveBeenCalledWith('t1', 'a');
    expect(sideQuestionsFor('t1')).toMatchObject([{ id: 'a', dismissed: true }]);
    settle();
    await dismissing;
  });

  it('puts the card back and says why when the dismissal fails', async () => {
    holdEvents('t1', [[1, asked('a', 'what is X?')]]);
    postDismissal.mockRejectedValue(new ApiError(502, 'engine down'));
    await dismissSideQuestion(sideQuestionsFor('t1')[0]);
    expect(sideQuestionsFor('t1')).toMatchObject([{ id: 'a', dismissed: false }]);
    expect(toast).toHaveBeenCalledWith('Could not dismiss the side question "what is X?": engine down', 'error');
  });

  it('drops a card the engine never recorded', async () => {
    post.mockRejectedValue(new ApiError(400, 'Side questions are not available in Codex threads.'));
    postDismissal.mockRejectedValue(new ApiError(404, 'No side question with this id was asked on this thread.'));
    await askSideQuestion('t1', 'q');
    await dismissSideQuestion(sideQuestionsFor('t1')[0]);
    expect(sideQuestionsFor('t1')).toEqual([]);
    expect(toast).not.toHaveBeenCalled();
  });

  it('records the dismissal of an answer whose events have not arrived yet', async () => {
    post.mockResolvedValue('a');
    postDismissal.mockResolvedValue();
    await askSideQuestion('t1', 'q');
    const [card] = sideQuestionsFor('t1');
    await dismissSideQuestion(card);
    expect(postDismissal).toHaveBeenCalledWith('t1', card.id);
    expect(sideQuestionsFor('t1')).toMatchObject([{ id: card.id, dismissed: true }]);
  });

  it('lets the next X dismiss a card reopened while its dismissal failed', async () => {
    holdEvents('t1', [[1, asked('a', 'q')]]);
    let fail!: (err: Error) => void;
    postDismissal.mockReturnValueOnce(new Promise((_, reject) => { fail = reject; }));
    const dismissing = dismissSideQuestion(sideQuestionsFor('t1')[0]);
    reopenSideQuestion('a');
    fail(new ApiError(502, 'engine down'));
    await dismissing;
    postDismissal.mockResolvedValue();
    await dismissSideQuestion(sideQuestionsFor('t1')[0]);
    expect(postDismissal).toHaveBeenCalledTimes(2);
  });

  it('reopens a dismissed card on this device only, and folds it again locally', async () => {
    holdEvents('t1', [[1, asked('a', 'q')], [2, { type: 'SideQuestionDismissed', side_question_id: 'a' }]]);
    reopenSideQuestion('a');
    const reopened = sideQuestionsFor('t1')[0];
    expect(reopened.dismissed).toBe(false);
    await dismissSideQuestion(reopened);
    expect(postDismissal).not.toHaveBeenCalled();
    expect(sideQuestionsFor('t1')[0].dismissed).toBe(true);
  });
});
