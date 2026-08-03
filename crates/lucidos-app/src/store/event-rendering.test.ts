import { describe, it, expect } from 'vitest';
import { hasVisibleLiveStep } from './event-rendering';
import type { ResponseEvent, StepOutcome } from './types';

const step = (outcome: StepOutcome): ResponseEvent => ({
  type: 'step',
  description: outcome === 'pending' ? 'Thinking' : 'Read file',
  outcome,
});
const text = (md: string): ResponseEvent => ({ type: 'text', md });

describe('hasVisibleLiveStep', () => {
  it('is true when steps are expanded, panel not collapsed, and a pending step is visible', () => {
    expect(hasVisibleLiveStep(true, false, [text('hi'), step('pending')])).toBe(true);
  });

  it('is false when steps are hidden, even if a pending step exists', () => {
    // The "Show steps" collapsed-toggle state: the live step is not on screen,
    // so the "Working" label must carry the shimmer instead.
    expect(hasVisibleLiveStep(false, false, [text('hi'), step('pending')])).toBe(false);
  });

  it('is false when the response panel is collapsed, even with a pending step', () => {
    // Collapse hides the whole steps body (only the header status shows), so the
    // pending step's shimmer is off screen — the label must carry it instead.
    expect(hasVisibleLiveStep(true, true, [text('hi'), step('pending')])).toBe(false);
  });

  it('is false when steps are expanded but every visible step has resolved', () => {
    expect(hasVisibleLiveStep(true, false, [step('success'), step('error'), text('done')])).toBe(false);
  });

  it('is false for an unfinished step: the turn died, nothing is running', () => {
    // A step killed mid-call is TERMINAL, not live. If it counted as a live
    // step it would suppress the "Working"/status shimmer on a dead turn and
    // (via `.running-shimmer`) animate a row nothing is working on.
    expect(hasVisibleLiveStep(true, false, [step('unfinished')])).toBe(false);
    expect(hasVisibleLiveStep(true, false, [step('success'), step('unfinished')])).toBe(false);
  });

  it('is false when there are no step events at all', () => {
    expect(hasVisibleLiveStep(true, false, [text('just text')])).toBe(false);
    expect(hasVisibleLiveStep(true, false, [])).toBe(false);
  });
});
