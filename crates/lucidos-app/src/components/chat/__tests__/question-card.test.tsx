import { describe, it, expect } from 'vitest';
import {
  questionModeLabel,
  ModeBadge,
  OptionIndicator,
  AnsweredBody,
  TerminatedQuestionBody,
} from '../QuestionCard';
import { vnodeToText } from './vnodeToText';

describe('questionModeLabel', () => {
  it('returns "Pick one" for single-select (multiSelect=false)', () => {
    expect(questionModeLabel(false)).toBe('Pick one');
  });

  it('returns "Pick one" when multiSelect is undefined (defaults to single)', () => {
    expect(questionModeLabel(undefined)).toBe('Pick one');
  });

  it('returns "Pick one or more" for multi-select', () => {
    expect(questionModeLabel(true)).toBe('Pick one or more');
  });

  it('returns "Suggested" for wake questions (optionCount=1), overriding multiSelect', () => {
    expect(questionModeLabel(false, 1)).toBe('Suggested');
    expect(questionModeLabel(true, 1)).toBe('Suggested');
    expect(questionModeLabel(undefined, 1)).toBe('Suggested');
  });

  it('falls back to "Pick one" / "Pick one or more" when optionCount > 1', () => {
    expect(questionModeLabel(false, 2)).toBe('Pick one');
    expect(questionModeLabel(true, 3)).toBe('Pick one or more');
  });
});

describe('ModeBadge', () => {
  it('renders the single-select label inside a .cc-question-mode-badge with the single modifier', () => {
    const text = vnodeToText(ModeBadge({ multiSelect: false }));
    expect(text).toContain('Pick one');
    expect(text).toContain('cc-question-mode-badge');
    expect(text).toContain('cc-question-mode-badge-single');
  });

  it('renders the multi-select label with the multi modifier', () => {
    const text = vnodeToText(ModeBadge({ multiSelect: true }));
    expect(text).toContain('Pick one or more');
    expect(text).toContain('cc-question-mode-badge-multi');
  });

  it('renders the wake-question variant (optionCount=1) with "Suggested", -suggested modifier, and no radio indicator', () => {
    const text = vnodeToText(ModeBadge({ multiSelect: false, optionCount: 1 }));
    expect(text).toContain('Suggested');
    expect(text).toContain('cc-question-mode-badge-suggested');
    expect(text).not.toContain('Pick one');
    expect(text).not.toContain('cc-question-mode-badge-single');
    expect(text).not.toContain('cc-question-option-indicator');
  });

  it('still uses the single variant when optionCount > 1', () => {
    const text = vnodeToText(ModeBadge({ multiSelect: false, optionCount: 2 }));
    expect(text).toContain('Pick one');
    expect(text).toContain('cc-question-mode-badge-single');
    expect(text).not.toContain('Suggested');
  });
});

describe('OptionIndicator', () => {
  it('renders a radio-style indicator for single-select (multiSelect=false)', () => {
    const text = vnodeToText(OptionIndicator({ multiSelect: false, selected: false }));
    expect(text).toContain('cc-question-option-indicator');
    expect(text).toContain('cc-question-option-indicator-radio');
    expect(text).not.toContain('cc-question-option-indicator-checkbox');
  });

  it('renders a checkbox-style indicator for multi-select', () => {
    const text = vnodeToText(OptionIndicator({ multiSelect: true, selected: false }));
    expect(text).toContain('cc-question-option-indicator-checkbox');
    expect(text).not.toContain('cc-question-option-indicator-radio');
  });

  it('marks the indicator as selected when selected=true', () => {
    const single = vnodeToText(OptionIndicator({ multiSelect: false, selected: true }));
    const multi = vnodeToText(OptionIndicator({ multiSelect: true, selected: true }));
    expect(single).toContain('cc-question-option-indicator-selected');
    expect(multi).toContain('cc-question-option-indicator-selected');
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
    expect(text).toContain('cc-question-cancel-picked');
    expect(text).toContain(' disabled');
    expect(text).toContain('Cancel');
    expect(text).toContain('✓');
    expect(text).not.toContain('cc-question-canceled-badge');
  });

  it('does not render the cancel-as-picked button for a Selected answer', () => {
    const text = vnodeToText(AnsweredBody({
      question: 'q',
      options: [{ id: 'a', label: 'A' }, { id: 'b', label: 'B' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).not.toContain('cc-question-cancel-picked');
    expect(text).not.toContain('action-btn-danger');
  });

  it('does not render the cancel-as-picked button for a FreeText answer', () => {
    const text = vnodeToText(AnsweredBody({
      question: 'q',
      options: [],
      multiSelect: false,
      resolved: { kind: 'FreeText', text: 'hello' },
    }));
    expect(text).not.toContain('cc-question-cancel-picked');
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
    expect(text).not.toContain('cc-question-cancel-picked');
    expect(text).not.toContain('cc-question-option-selected');
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
    expect(text).toContain('cc-question-mode-badge-multi');
  });

  it('renders only the question text when the question had no options (freetext-only ask)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      question: 'What do you think?',
      options: [],
      multiSelect: false,
    }));
    expect(text).toContain('What do you think?');
    expect(text).not.toContain('cc-question-mode-badge');
    expect(text).not.toContain(' disabled');
  });
});

describe('question text — URL linkification', () => {
  const QUESTION =
    'Draft PR #1488 is ready: https://github.com/m10s-green/user-acquisition/pull/1488 — mark it ready?';

  it('renders a bare URL in the question as a clickable new-tab link (terminated body)', () => {
    const text = vnodeToText(TerminatedQuestionBody({
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
    }));
    expect(text).toContain(
      '<a href="https://github.com/m10s-green/user-acquisition/pull/1488" target="_blank" rel="noopener">',
    );
  });

  it('renders the URL as a link in the answered body too', () => {
    const text = vnodeToText(AnsweredBody({
      question: QUESTION,
      options: [{ id: 'a', label: 'Yes' }],
      multiSelect: false,
      resolved: { kind: 'Selected', option_id: 'a' },
    }));
    expect(text).toContain('href="https://github.com/m10s-green/user-acquisition/pull/1488"');
    expect(text).toContain('target="_blank"');
  });
});
