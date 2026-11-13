import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import {
  dividerBodyIsSuppressed,
  questionDividerResolution,
  type StoredEvent,
} from '../../../store/thread-events';
import type { ResponseEvent } from '../../../store/types';
import { makeExchange, step } from '../../../store/__tests__/fixtures';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

const askedEvent: StoredEvent = {
  type: 'UserQuestionAsked',
  tool_use_id: 'tu_div',
  cc_session_id: 'sess',
  question: 'Pick:',
  options: [{ id: 'opt-0', label: 'A' }],
} as StoredEvent;

describe('questionDividerResolution', () => {
  it('reports canceled when the divider\'s UserQuestionAnswered carries kind: Canceled', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Canceled' },
      } as StoredEvent),
    ]);
    expect(questionDividerResolution(ex)).toBe('canceled');
  });

  // A follow-up that could not be the answer resolves the question, which is
  // what releases the parked agent. Its next turn belongs to the follow-up's
  // own exchange, so this divider has no body coming either.
  it('reports superseded when a follow-up replaced the question', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Superseded' },
      } as StoredEvent),
    ]);
    expect(questionDividerResolution(ex)).toBe('superseded');
  });

  it('holds even when an empty CodingAgentPromptSent placeholder trails the answer', () => {
    // Some cancel paths land the cancel-stamp alongside a stray empty marker;
    // the divider gate must hold regardless of whether the marker shows up.
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Canceled' },
      } as StoredEvent),
      step(2, { type: 'CodingAgentPromptSent', text: '' } as StoredEvent),
    ]);
    expect(questionDividerResolution(ex)).toBe('canceled');
  });

  it('is null for a Selected answer, since CC resumes and the panel stays', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      } as StoredEvent),
    ]);
    expect(questionDividerResolution(ex)).toBeNull();
  });

  it('is null for a FreeText answer', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'FreeText', text: 'red' },
      } as StoredEvent),
    ]);
    expect(questionDividerResolution(ex)).toBeNull();
  });

  it('is null while the question is still unanswered', () => {
    const ex = makeExchange(askedEvent, []);
    expect(questionDividerResolution(ex)).toBeNull();
  });

  it('is null for non-divider exchanges (e.g. MessageReceived) even with a stray Canceled answer', () => {
    const ex = makeExchange(
      { type: 'MessageReceived', text: 'hi' } as StoredEvent,
      [
        step(1, {
          type: 'UserQuestionAnswered',
          tool_use_id: 'tu_div',
          answer: { kind: 'Canceled' },
        } as StoredEvent),
      ],
    );
    expect(questionDividerResolution(ex)).toBeNull();
  });
});

/** The suppression itself. It is `questionDividerResolution` plus one carve-out:
 *  a body carrying what Lucidos said out loud is the transcript of a call, and
 *  nothing else in the app draws one. The caller's own words are not looked
 *  for, since those open an exchange of their own. */
describe('dividerBodyIsSuppressed', () => {
  const canceled = () =>
    makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Canceled' },
      } as StoredEvent),
    ]);

  it('hides the body of a typed thread canceled divider', () => {
    expect(dividerBodyIsSuppressed(canceled(), [])).toBe(true);
  });

  it('keeps a body carrying what Lucidos said out loud', () => {
    const events: ResponseEvent[] = [
      { type: 'spoken_reply', text: 'The codebase is clean.', interrupted: false },
    ];
    expect(dividerBodyIsSuppressed(canceled(), events)).toBe(false);
  });

  // The carve-out is a SPOKEN row, not renderable content. A cancel step and a
  // step row land in every canceled divider, so the wider test would un-hide
  // the body on every typed thread.
  it('still hides a body whose only content is an ordinary step', () => {
    const events: ResponseEvent[] = [
      { type: 'step', description: 'Ran a query', outcome: 'success' },
    ];
    expect(dividerBodyIsSuppressed(canceled(), events)).toBe(true);
  });

  it('is false for a divider still awaiting its answer, whatever the body', () => {
    expect(dividerBodyIsSuppressed(makeExchange(askedEvent, []), [])).toBe(false);
  });
});

describe('ChatExchange wires the suppression into showResponsePanel', () => {
  it('imports the helper from ../../store/thread-events', () => {
    expect(source).toMatch(/dividerBodyIsSuppressed/);
    expect(source).toMatch(/from\s+['"]\.\.\/\.\.\/store\/thread-events['"]/);
  });

  it('showResponsePanel includes !isUnansweredDivider', () => {
    // ChatExchange is now `memo(ChatExchangeImpl, …)`; the body lives in the
    // `function ChatExchangeImpl(...) { … }` declaration above the export.
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    expect(fnMatch, 'ChatExchangeImpl function not found').not.toBeNull();
    const fn = fnMatch![0];
    // The events list, not the exchange alone: the carve-out is decided by
    // what the body would DRAW, and only the rendered list knows that.
    expect(fn).toMatch(
      /isUnansweredDivider\s*=\s*dividerBodyIsSuppressed\(exchange,\s*events\)/,
    );
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*!isUnansweredDivider/);
  });
});
