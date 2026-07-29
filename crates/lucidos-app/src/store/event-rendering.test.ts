import { describe, it, expect } from 'vitest';
import { hasVisibleLiveStep } from './event-rendering';
import type { ResponseEvent } from './types';

const step = (success: boolean | null): ResponseEvent => ({
  type: 'step',
  description: success === null ? 'Thinking' : 'Read file',
  success,
});
const text = (md: string): ResponseEvent => ({ type: 'text', md });

describe('hasVisibleLiveStep', () => {
  it('is true when steps are expanded, panel not collapsed, and a pending step is visible', () => {
    expect(hasVisibleLiveStep(true, false, [text('hi'), step(null)])).toBe(true);
  });

  it('is false when steps are hidden, even if a pending step exists', () => {
    // The "Show steps" collapsed-toggle state: the live step is not on screen,
    // so the "Working" label must carry the shimmer instead.
    expect(hasVisibleLiveStep(false, false, [text('hi'), step(null)])).toBe(false);
  });

  it('is false when the response panel is collapsed, even with a pending step', () => {
    // Collapse hides the whole steps body (only the header status shows), so the
    // pending step's shimmer is off screen — the label must carry it instead.
    expect(hasVisibleLiveStep(true, true, [text('hi'), step(null)])).toBe(false);
  });

  it('is false when steps are expanded but every visible step has resolved', () => {
    expect(hasVisibleLiveStep(true, false, [step(true), step(false), text('done')])).toBe(false);
  });

  it('is false when there are no step events at all', () => {
    expect(hasVisibleLiveStep(true, false, [text('just text')])).toBe(false);
    expect(hasVisibleLiveStep(true, false, [])).toBe(false);
  });
});
