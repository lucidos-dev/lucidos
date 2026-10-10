/**
 * Tests the reveal animation eligibility logic for ThreadView.
 *
 * shouldRevealThread determines eligibility — the actual animation also
 * requires revealOnFocus to be true (checked in the layout effect, not here).
 *
 * Eligible when:
 * - Thread ID is set
 * - Not currently animating (FLIP)
 * - Content is available
 * - Thread hasn't already been revealed
 *
 * Not eligible:
 * - On same-thread re-renders
 * - While FLIP animation is in progress
 * - While events haven't loaded yet
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { shouldRevealThread, commitReveal, resetRevealTracking } from '../ThreadView';

describe('thread reveal animation eligibility', () => {
  beforeEach(() => {
    resetRevealTracking();
  });

  it('eligible on first thread view', () => {
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
  });

  it('not eligible on same thread re-render', () => {
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
    commitReveal('thread-a');
    expect(shouldRevealThread('thread-a', false, true)).toBe(false);
  });

  it('eligible on thread switch A→B', () => {
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
    commitReveal('thread-a');
    expect(shouldRevealThread('thread-b', false, true)).toBe(true);
  });

  it('eligible on thread switch after unmount', () => {
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
    commitReveal('thread-a');
    resetRevealTracking(); // simulates unmount
    expect(shouldRevealThread('thread-b', false, true)).toBe(true);
  });

  it('eligible when returning to same thread after unmount', () => {
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
    commitReveal('thread-a');
    resetRevealTracking(); // simulates unmount
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
  });

  it('not eligible while animating (FLIP in progress)', () => {
    expect(shouldRevealThread('thread-a', true, true)).toBe(false);
  });

  it('eligible after FLIP completes', () => {
    expect(shouldRevealThread('thread-a', true, true)).toBe(false);
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
  });

  it('not eligible while events not loaded', () => {
    expect(shouldRevealThread('thread-a', false, false)).toBe(false);
  });

  it('eligible once events load', () => {
    expect(shouldRevealThread('thread-a', false, false)).toBe(false);
    expect(shouldRevealThread('thread-a', false, true)).toBe(true);
  });

  it('not eligible for null threadId', () => {
    expect(shouldRevealThread(null, false, true)).toBe(false);
  });

  it('rapid thread switches all eligible', () => {
    expect(shouldRevealThread('a', false, true)).toBe(true);
    commitReveal('a');
    expect(shouldRevealThread('b', false, true)).toBe(true);
    commitReveal('b');
    expect(shouldRevealThread('c', false, true)).toBe(true);
  });
});
