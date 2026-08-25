// @vitest-environment jsdom
// This file renders markdown, and the sanitizer runs on a real DOM.
// The default `node` environment has none.
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import * as QuestionCardModule from '../QuestionCard';
import {
  OptionIndicator,
  AnsweredBody,
  TerminatedQuestionBody,
} from '../QuestionCard';
import { PLACEHOLDER_ANSWERING } from '../prompt-input-helpers';
import { ANSWER_CANCEL_TOOLTIP } from '../PromptInput';
import { vnodeToText } from './vnodeToText';

describe('OptionIndicator', () => {
  it('renders a radio-style indicator for single-select (multiSelect=false)', () => {
    const text = vnodeToText(OptionIndicator({ multiSelect: false, selected: false }));
    expect(text).toContain('question-option-indicator');
    expect(text).toContain('question-option-indicator-radio');
    expect(text).not.toContain('question-option-indicator-checkbox');
  });

  it('renders a checkbox-style indicator for multi-select', () => {
    const text = vnodeToText(OptionIndicator({ multiSelect: true, selected: false }));
    expect(text).toContain('question-option-indicator-checkbox');
    expect(text).not.toContain('question-option-indicator-radio');
  });

  it('marks the indicator as selected when selected=true', () => {
    const single = vnodeToText(OptionIndicator({ multiSelect: false, selected: true }));
    const multi = vnodeToText(OptionIndicator({ multiSelect: true, selected: true }));
    expect(single).toContain('question-option-indicator-selected');
    expect(multi).toContain('question-option-indicator-selected');
  });
});

// The agent used to author an "Other, I'll type it" option because nothing on
// a card with options said the two real escapes existed. Naming them is what
// fixed that, but the card is the wrong surface for it: a guide line under the
// options sat a few pixels above the textarea it pointed at, and said the same
// thing that textarea's own placeholder says. The prompt row owns both halves
// now, one on each control: the textarea's placeholder names typing, the Cancel
// button's tooltip names Cancel.
describe('the card carries no hint of its own', () => {
  const SOURCE = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../QuestionCard.tsx'),
    'utf8',
  );

  it('renders no hint element and exports no hint symbol', () => {
    expect(SOURCE).not.toContain('class="question-hint"');
    expect(Object.keys(QuestionCardModule).filter(k => /hint/i.test(k))).toEqual([]);
  });

  // "custom answer", not "your answer": the placeholder has to read as a peer of
  // the card's options rather than as composer chrome. Short and unpunctuated
  // for the same reason, since a placeholder that wraps to two lines of grey
  // text at 125% scale in monospace is the first thing the eye skips.
  it('leaves the typing escape to the prompt placeholder, in one short line', () => {
    expect(PLACEHOLDER_ANSWERING).toContain('Type');
    expect(PLACEHOLDER_ANSWERING).toContain('custom answer');
    expect(PLACEHOLDER_ANSWERING.length).toBeLessThan(32);
    expect(PLACEHOLDER_ANSWERING).not.toContain('.');
  });

  // The other half of the pair: what the red button does to a pending question
  // is spelled out nowhere else, so the tooltip carries it.
  it('leaves the Cancel escape to the prompt row Cancel tooltip', () => {
    expect(ANSWER_CANCEL_TOOLTIP).toContain('Cancel');
    expect(ANSWER_CANCEL_TOOLTIP).toContain('ask something else');
  });

  // Pin the WIRING, not just the wording. While both halves lived in one string
  // the placeholder assertion covered them together; split across two controls,
  // a refactor that drops the tooltip leaves the constant (and the assertion
  // above) intact while the escape goes unnamed on screen. Source-scan for the
  // same reason as prompt-answer-no-images.test.ts: mounting PromptInput drags
  // in every chat signal and store.
  it('wires that tooltip onto both Cancel controls in PromptInput', () => {
    const promptSource = readFileSync(
      resolve(dirname(fileURLToPath(import.meta.url)), '../PromptInput.tsx'),
      'utf8',
    );
    // Lone Cancel: question card gets the wording, a permission card keeps Stop.
    expect(promptSource).toMatch(
      /data-tooltip=\{answeringQuestionCard\s*\?\s*ANSWER_CANCEL_TOOLTIP\s*:\s*'Stop'\}/,
    );
    // Multi-select split button: that path only exists with a question pending.
    expect(promptSource).toMatch(/tooltip:\s*ANSWER_CANCEL_TOOLTIP/);
  });
});

describe('AnsweredBody — Canceled state', () => {
  it('renders a disabled red Cancel button instead of the orange canceled badge', () => {
    const text = vnodeToText(AnsweredBody({
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }],
      multiSelect: false,
      resolved: { kind: 'Canceled' },
    }));
    expect(text).toContain('action-btn');
    expect(text).toContain('action-btn-danger');
    expect(text).toContain('question-cancel-picked');
    expect(text).toContain(' disabled');
    expect(text).toContain('Cancel');
    expect(text).toContain('✓');
    expect(text).not.toContain('question-canceled-badge');
  });

  // Nobody chose this outcome, so the card states it rather than rendering a
  // picked affordance. Reading "Canceled" here would blame the user for
  // dismissing a question they actually replied past.
  it('renders a plain note for a Superseded answer, never the Cancel affordance', () => {
    const text = vnodeToText(AnsweredBody({
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }],
      multiSelect: false,
      resolved: { kind: 'Superseded' },
    }));
    expect(text).toContain('question-superseded-note');
    expect(text).toContain('Replaced by your next message');
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('Cancel');
  });

  it('does not render the cancel-as-picked button for a Selected answer', () => {
    const text = vnodeToText(AnsweredBody({
      toolUseId: 'tool-1',
      question: 'q',
      options: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('action-btn-danger');
  });

  it('does not render the cancel-as-picked button for a FreeText answer', () => {
    const text = vnodeToText(AnsweredBody({
      toolUseId: 'tool-1',
      question: 'q',
      options: [],
      multiSelect: false,
      resolved: { kind: 'FreeText', text: 'hello' },
    }));
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('action-btn-danger');
    expect(text).toContain('hello');
  });
});

describe('TerminatedQuestionBody', () => {
  it('renders every option as a disabled button (single-select)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      question: 'Pick a flavor',
      options: [
        { id: 'a', label: 'Vanilla' },
        { id: 'b', label: 'Chocolate' },
      ],
      multiSelect: false,
    }));
    expect(text).toContain('Pick a flavor');
    expect(text).toContain('Vanilla');
    expect(text).toContain('Chocolate');
    const disabledCount = (text.match(/ disabled/g) ?? []).length;
    expect(disabledCount).toBe(2);
    // The user never picked — no cancel-picked / selected affordance.
    expect(text).not.toContain('question-cancel-picked');
    expect(text).not.toContain('question-option-selected');
  });

  it('renders every option as a disabled button (multi-select)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      question: 'Pick any',
      options: [
        { id: 'a', label: 'A' },
        { id: 'b', label: 'B' },
        { id: 'c', label: 'C' },
      ],
      multiSelect: true,
    }));
    const disabledCount = (text.match(/ disabled/g) ?? []).length;
    expect(disabledCount).toBe(3);
  });

  it('renders only the question text when the question had no options (freetext-only ask)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      question: 'What do you think?',
      options: [],
      multiSelect: false,
    }));
    expect(text).toContain('What do you think?');
    expect(text).not.toContain(' disabled');
  });
});

describe('question text — URL linkification', () => {
  const QUESTION =
    'Draft PR #1488 is ready: https://github.com/example-org/example-repo/pull/1488 — mark it ready?';

  it('renders a bare URL in the question as a clickable new-tab link (terminated body)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
    }));
    expect(text).toContain(
      '<a href="https://github.com/example-org/example-repo/pull/1488" target="_blank" rel="noopener">',
    );
  });

  it('renders the URL as a link in the answered body too', () => {
    const text = vnodeToText(AnsweredBody({
      toolUseId: 'tool-1',
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).toContain('href="https://github.com/example-org/example-repo/pull/1488"');
    expect(text).toContain('target="_blank"');
  });
});
