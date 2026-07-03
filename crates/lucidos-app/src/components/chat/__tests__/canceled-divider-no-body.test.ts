import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import {
  isCanceledQuestionDivider,
  type StoredEvent,
} from '../../../store/thread-events';
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

describe('isCanceledQuestionDivider', () => {
  it('is true when the divider\'s UserQuestionAnswered carries kind: Canceled', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Canceled' },
      } as StoredEvent),
    ]);
    expect(isCanceledQuestionDivider(ex)).toBe(true);
  });

  it('is true even when an empty CodingAgentPromptSent placeholder trails the answer', () => {
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
    expect(isCanceledQuestionDivider(ex)).toBe(true);
  });

  it('is false for a Selected answer (CC will resume — keep the panel)', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'Selected', option_id: 'opt-0' },
      } as StoredEvent),
    ]);
    expect(isCanceledQuestionDivider(ex)).toBe(false);
  });

  it('is false for a FreeText answer', () => {
    const ex = makeExchange(askedEvent, [
      step(1, {
        type: 'UserQuestionAnswered',
        tool_use_id: 'tu_div',
        answer: { kind: 'FreeText', text: 'red' },
      } as StoredEvent),
    ]);
    expect(isCanceledQuestionDivider(ex)).toBe(false);
  });

  it('is false while the question is still unanswered', () => {
    const ex = makeExchange(askedEvent, []);
    expect(isCanceledQuestionDivider(ex)).toBe(false);
  });

  it('is false for non-divider exchanges (e.g. MessageReceived) even with a stray Canceled answer', () => {
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
    expect(isCanceledQuestionDivider(ex)).toBe(false);
  });
});

describe('ChatExchange wires isCanceledQuestionDivider into showResponsePanel', () => {
  it('imports the helper from ../../store/thread-events', () => {
    expect(source).toMatch(/isCanceledQuestionDivider/);
    expect(source).toMatch(/from\s+['"]\.\.\/\.\.\/store\/thread-events['"]/);
  });

  it('showResponsePanel includes !isCanceledDivider', () => {
    // ChatExchange is now `memo(ChatExchangeImpl, …)`; the body lives in the
    // `function ChatExchangeImpl(...) { … }` declaration above the export.
    const fnMatch = source.match(/function ChatExchangeImpl[\s\S]*?^\}/m);
    expect(fnMatch, 'ChatExchangeImpl function not found').not.toBeNull();
    const fn = fnMatch![0];
    expect(fn).toMatch(/isCanceledDivider\s*=\s*isCanceledQuestionDivider\(exchange\)/);
    expect(fn).toMatch(/showResponsePanel\s*=[^;]*!isCanceledDivider/);
  });
});
