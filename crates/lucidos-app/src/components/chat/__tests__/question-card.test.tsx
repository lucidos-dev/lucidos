import { describe, it, expect } from 'vitest';
import {
  OptionIndicator,
  AnsweredBody,
  TerminatedQuestionBody,
} from '../QuestionCard';
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

describe('AnsweredBody — Canceled state', () => {
  it('renders a disabled red Cancel button instead of the orange canceled badge', () => {
    const text = vnodeToText(AnsweredBody({
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

  it('does not render the cancel-as-picked button for a Selected answer', () => {
    const text = vnodeToText(AnsweredBody({
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
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).toContain('href="https://github.com/example-org/example-repo/pull/1488"');
    expect(text).toContain('target="_blank"');
  });
});
