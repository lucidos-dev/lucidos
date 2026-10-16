import { describe, it, expect } from 'vitest';
import { computeAnswerActionMode } from '../prompt-input-helpers';

const base = {
  pendingMultiQ: false,
  hasContent: false,
  isCanceling: false,
};

describe('computeAnswerActionMode', () => {
  it('cancel when nothing is submittable (single-select / permission / empty freetext)', () => {
    expect(computeAnswerActionMode(base)).toBe('cancel');
  });

  it('submit when a freetext/custom answer has been typed', () => {
    expect(computeAnswerActionMode({ ...base, hasContent: true })).toBe('submit');
  });

  it('multi for a pending multi-select question (the only state that needs the caret)', () => {
    expect(computeAnswerActionMode({ ...base, pendingMultiQ: true })).toBe('multi');
  });

  // Multi-select always shows Submit (disabled at zero), so typed content does
  // not change the mode — it still routes through the split button.
  it('multi wins over typed content', () => {
    expect(computeAnswerActionMode({ ...base, pendingMultiQ: true, hasContent: true })).toBe('multi');
  });

  it('canceling overrides every other state once a Cancel is in flight', () => {
    expect(computeAnswerActionMode({ ...base, isCanceling: true })).toBe('canceling');
    expect(computeAnswerActionMode({ ...base, isCanceling: true, hasContent: true })).toBe('canceling');
    expect(computeAnswerActionMode({ ...base, isCanceling: true, pendingMultiQ: true })).toBe('canceling');
  });
});
