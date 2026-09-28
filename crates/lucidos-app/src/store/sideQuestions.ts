import { signal } from '@preact/signals';
import { askSideQuestion as postSideQuestion, ApiError } from '../api/client';
import { errorDetail } from '../utils/errorDetail';

/** A `/btw` side question and where its answer stands. Held in memory only:
 *  the engine records nothing, so a reload or another device shows no card
 *  (ADR 0318). */
export type SideQuestion =
  | { id: string; threadId: string; question: string; status: 'pending' }
  | { id: string; threadId: string; question: string; status: 'answered'; answer: string }
  | { id: string; threadId: string; question: string; status: 'failed'; error: string };

/** The menu entry and composer prefix for a side question. */
export const SIDE_QUESTION_COMMAND = 'btw';

/** Every side question on screen, oldest first. */
export const sideQuestions = signal<readonly SideQuestion[]>([]);

/** The question in `message` when it is a side question (`/btw` as its first
 *  word), else `null`. An empty string is a bare `/btw`. Mirrors the engine's
 *  `is_side_question`, which refuses the same text on the normal chat route. */
export function sideQuestionText(message: string): string | null {
  const match = /^\s*\/btw(?:\s+([\s\S]*))?$/.exec(message);
  return match ? (match[1] ?? '').trim() : null;
}

/** The filter text that turns into a side question as the user types it in
 *  the command menu: `btw` and a space. */
export function isSideQuestionFilter(filter: string): boolean {
  return /^btw\s/.test(filter);
}

/** The composer text once the command menu hands back `handoff` (`/btw …`).
 *  A draft is never lost: it becomes the question. */
export function withSideQuestionPrefix(handoff: string, draft: string): string {
  if (draft.trim() === '') return handoff;
  if (sideQuestionText(draft) !== null) return draft;
  return `${handoff.trimEnd()} ${draft.trimStart()}`;
}

/** What the composer does with a submitted message. */
export type SideQuestionRoute =
  /** Not a side question, or not a coding-agent thread: send it as usual. */
  | { kind: 'message' }
  /** A side question that cannot be asked here. The draft stays. */
  | { kind: 'refuse'; toast: string }
  | { kind: 'ask'; question: string };

export const SIDE_QUESTION_NOT_STARTED =
  'Side questions work once this thread has started. Send a normal message first.';
export const SIDE_QUESTION_TEXT_ONLY = 'Side questions are text only. Remove the images to ask.';
export const SIDE_QUESTION_EMPTY = 'Type a question after /btw.';

/** Route a submit. In a coding-agent thread a side question never goes to the
 *  main session: it is asked, or refused with the draft kept. A Codex thread
 *  is asked too, and the engine's refusal lands on the card. */
export function routeSideQuestion(
  message: string,
  thread: { codingAgent: boolean; started: boolean; hasImages: boolean },
): SideQuestionRoute {
  const question = sideQuestionText(message);
  if (question === null || !thread.codingAgent) return { kind: 'message' };
  if (!thread.started) return { kind: 'refuse', toast: SIDE_QUESTION_NOT_STARTED };
  if (thread.hasImages) return { kind: 'refuse', toast: SIDE_QUESTION_TEXT_ONLY };
  if (question === '') return { kind: 'refuse', toast: SIDE_QUESTION_EMPTY };
  return { kind: 'ask', question };
}

export function sideQuestionsFor(all: readonly SideQuestion[], threadId: string): SideQuestion[] {
  return all.filter((q) => q.threadId === threadId);
}

let nextId = 0;

/** Ask a side question and show its card. Never throws: a failure lands on
 *  the card. A card dismissed while pending drops its answer. */
export async function askSideQuestion(threadId: string, question: string): Promise<void> {
  const id = `side-question-${++nextId}`;
  sideQuestions.value = [...sideQuestions.value, { id, threadId, question, status: 'pending' }];
  let settled: SideQuestion;
  try {
    const answer = await postSideQuestion(threadId, question);
    settled = { id, threadId, question, status: 'answered', answer };
  } catch (err) {
    const error = err instanceof ApiError ? err.reason : errorDetail(err);
    settled = { id, threadId, question, status: 'failed', error };
  }
  sideQuestions.value = sideQuestions.value.map((q) => (q.id === id ? settled : q));
}

export function dismissSideQuestion(id: string): void {
  sideQuestions.value = sideQuestions.value.filter((q) => q.id !== id);
}
