import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const post = vi.fn<(threadId: string, question: string) => Promise<string>>();
vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>();
  return { ...actual, askSideQuestion: (t: string, q: string) => post(t, q) };
});

import { ApiError } from '../api/client';
import {
  SIDE_QUESTION_EMPTY,
  SIDE_QUESTION_NOT_STARTED,
  SIDE_QUESTION_TEXT_ONLY,
  askSideQuestion,
  dismissSideQuestion,
  isSideQuestionFilter,
  routeSideQuestion,
  sideQuestionText,
  sideQuestions,
  sideQuestionsFor,
  withSideQuestionPrefix,
} from './sideQuestions';

const STARTED_CC = { codingAgent: true, started: true, hasImages: false };

describe('sideQuestionText', () => {
  it('reads /btw as the first word, like the engine', () => {
    expect(sideQuestionText('/btw what is X?')).toBe('what is X?');
    expect(sideQuestionText('  /btw  spaced  ')).toBe('spaced');
    expect(sideQuestionText('/btw\nline one\nline two')).toBe('line one\nline two');
    expect(sideQuestionText('/btw')).toBe('');
  });

  it('leaves every other message alone', () => {
    for (const text of ['/btwx q', 'btw q', 'fix the /btw route', '/compact', 'hello', '']) {
      expect(sideQuestionText(text), text).toBeNull();
    }
  });
});

describe('isSideQuestionFilter', () => {
  it('fires once btw is followed by a space', () => {
    expect(isSideQuestionFilter('btw ')).toBe(true);
    expect(isSideQuestionFilter('btw what')).toBe(true);
    expect(isSideQuestionFilter('btw')).toBe(false);
    expect(isSideQuestionFilter('bt')).toBe(false);
    expect(isSideQuestionFilter('model')).toBe(false);
  });
});

describe('withSideQuestionPrefix', () => {
  it('fills an empty composer with the handed-back text', () => {
    expect(withSideQuestionPrefix('/btw ', '')).toBe('/btw ');
    expect(withSideQuestionPrefix('/btw ', '  \n')).toBe('/btw ');
  });

  it('keeps a draft, turning it into the question', () => {
    expect(withSideQuestionPrefix('/btw ', 'why is the build slow?')).toBe('/btw why is the build slow?');
    expect(withSideQuestionPrefix('/btw what', '  and why')).toBe('/btw what and why');
  });

  it('leaves a draft that already is a side question alone', () => {
    expect(withSideQuestionPrefix('/btw ', '/btw what is X?')).toBe('/btw what is X?');
  });
});

describe('routeSideQuestion', () => {
  it('asks a side question in a started coding-agent thread', () => {
    expect(routeSideQuestion('/btw what is X?', STARTED_CC)).toEqual({ kind: 'ask', question: 'what is X?' });
  });

  it('sends a normal message and other slash commands as usual', () => {
    expect(routeSideQuestion('please fix the tests', STARTED_CC)).toEqual({ kind: 'message' });
    expect(routeSideQuestion('/compact', STARTED_CC)).toEqual({ kind: 'message' });
  });

  it('leaves /btw alone outside coding-agent threads', () => {
    expect(routeSideQuestion('/btw q', { ...STARTED_CC, codingAgent: false })).toEqual({ kind: 'message' });
  });

  it('refuses, keeping the draft, where it cannot be asked', () => {
    expect(routeSideQuestion('/btw q', { ...STARTED_CC, started: false }))
      .toEqual({ kind: 'refuse', toast: SIDE_QUESTION_NOT_STARTED });
    expect(routeSideQuestion('/btw q', { ...STARTED_CC, hasImages: true }))
      .toEqual({ kind: 'refuse', toast: SIDE_QUESTION_TEXT_ONLY });
    expect(routeSideQuestion('/btw', STARTED_CC)).toEqual({ kind: 'refuse', toast: SIDE_QUESTION_EMPTY });
  });
});

describe('askSideQuestion', () => {
  beforeEach(() => {
    sideQuestions.value = [];
    post.mockReset();
  });
  afterEach(() => {
    sideQuestions.value = [];
  });

  it('shows a pending card, then the answer', async () => {
    let resolve!: (answer: string) => void;
    post.mockReturnValue(new Promise((r) => { resolve = r; }));
    const asked = askSideQuestion('t1', 'what is X?');
    expect(post).toHaveBeenCalledWith('t1', 'what is X?');
    expect(sideQuestions.value).toMatchObject([{ threadId: 't1', question: 'what is X?', status: 'pending' }]);
    resolve('X is a letter.');
    await asked;
    expect(sideQuestions.value).toMatchObject([{ status: 'answered', answer: 'X is a letter.' }]);
  });

  it("puts the engine's refusal on the card, for a Codex thread", async () => {
    post.mockRejectedValue(new ApiError(400, 'Side questions are not available in Codex threads.'));
    await askSideQuestion('codex-thread', 'q');
    expect(sideQuestions.value).toMatchObject([
      { status: 'failed', error: 'Side questions are not available in Codex threads.' },
    ]);
  });

  it('drops the answer of a card dismissed while pending', async () => {
    let resolve!: (answer: string) => void;
    post.mockReturnValue(new Promise((r) => { resolve = r; }));
    const asked = askSideQuestion('t1', 'q');
    dismissSideQuestion(sideQuestions.value[0].id);
    resolve('late');
    await asked;
    expect(sideQuestions.value).toEqual([]);
  });

  it('stacks cards and keeps each thread to its own', async () => {
    post.mockResolvedValue('a');
    await askSideQuestion('t1', 'one');
    await askSideQuestion('t2', 'two');
    await askSideQuestion('t1', 'three');
    expect(sideQuestionsFor(sideQuestions.value, 't1').map((q) => q.question)).toEqual(['one', 'three']);
  });
});
