import { describe, it, expect } from 'vitest';
import { computeAnswerActionMode } from '../prompt-input-helpers';

const single = { isCanceling: false, hasContent: false, multi: null };
const emptyMulti = { pickedCount: 0, text: '', imageCount: 0 };

describe('computeAnswerActionMode', () => {
  it('cancel on an empty multi-select card: no pick, nothing typed', () => {
    expect(computeAnswerActionMode({ ...single, multi: emptyMulti })).toBe('cancel');
  });

  it('cancel while the box holds only whitespace', () => {
    expect(computeAnswerActionMode({ ...single, multi: { ...emptyMulti, text: '  ' } })).toBe('cancel');
  });

  it('submit once one option is picked', () => {
    expect(computeAnswerActionMode({ ...single, multi: { ...emptyMulti, pickedCount: 1 } })).toBe('submit');
  });

  it('submit once text is typed on a multi-select card', () => {
    expect(computeAnswerActionMode({ ...single, multi: { ...emptyMulti, text: 'mine' } })).toBe('submit');
  });

  it('submit with images alone on a multi-select card', () => {
    expect(computeAnswerActionMode({ ...single, multi: { ...emptyMulti, imageCount: 1 } })).toBe('submit');
  });

  it('cancel on a single-pick or permission card with an empty box', () => {
    expect(computeAnswerActionMode(single)).toBe('cancel');
  });

  it('submit on a single-pick card once an answer is typed', () => {
    expect(computeAnswerActionMode({ ...single, hasContent: true })).toBe('submit');
  });

  it('canceling overrides every other state once a Cancel is in flight', () => {
    expect(computeAnswerActionMode({ ...single, isCanceling: true })).toBe('canceling');
    expect(computeAnswerActionMode({ ...single, isCanceling: true, hasContent: true })).toBe('canceling');
    expect(computeAnswerActionMode({ ...single, isCanceling: true, multi: { ...emptyMulti, pickedCount: 2 } })).toBe('canceling');
  });
});
