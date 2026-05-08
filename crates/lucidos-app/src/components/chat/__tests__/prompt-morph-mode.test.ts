import { describe, it, expect } from 'vitest';
import { computeMorphMode } from '../PromptInput';

const base = {
  hasContent: false,
  cancelTargetId: null as string | null,
  isCanceling: false,
  hasBannerOrSectionButtons: false,
};

describe('computeMorphMode', () => {
  it('placeholder when nothing else owns the slot', () => {
    expect(computeMorphMode(base)).toBe('placeholder');
  });

  it('hidden when banner or section buttons own the slot', () => {
    expect(computeMorphMode({ ...base, hasBannerOrSectionButtons: true })).toBe('hidden');
  });

  it('send when user typed text and nothing blocks the turn', () => {
    expect(computeMorphMode({ ...base, hasContent: true })).toBe('send');
  });

  it('cancel when a cancel target exists and slot is otherwise free', () => {
    expect(computeMorphMode({ ...base, cancelTargetId: 't1' })).toBe('cancel');
  });

  it('canceling when cancel target exists and click already fired', () => {
    expect(computeMorphMode({ ...base, cancelTargetId: 't1', isCanceling: true })).toBe('canceling');
  });

  // hasContent wins over cancelTargetId — once the user starts typing, the
  // morph flips back to Send so the follow-up path stays accessible.
  it('send wins over cancel when user has typed text', () => {
    expect(computeMorphMode({ ...base, hasContent: true, cancelTargetId: 't1' })).toBe('send');
  });
});
