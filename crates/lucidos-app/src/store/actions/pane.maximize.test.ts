import { describe, it, expect } from 'vitest';
import { computeMaximizeRatio, computeDrawerShortcutAction } from './pane';
import { DEFAULT_SPLIT_RATIO } from '../../components/layout/splitHelpers';

describe('computeDrawerShortcutAction (⌘⇧1 three-stage)', () => {
  it('hidden drawer → open + focus', () => {
    // Hidden covers a closed drawer, a Conversation side that is collapsed, and
    // a drawer the layout yielded: none of them is drawn.
    expect(computeDrawerShortcutAction(false, false)).toBe('open-focus');
    expect(computeDrawerShortcutAction(false, true)).toBe('open-focus');
  });
  it('visible but not the focused pane → focus', () => {
    expect(computeDrawerShortcutAction(true, false)).toBe('focus');
  });
  it('visible and already focused → close', () => {
    expect(computeDrawerShortcutAction(true, true)).toBe('close');
  });
});

describe('computeMaximizeRatio (⌘⇧↵ toggle)', () => {
  it('thread focused, normal split → maximize Threads pane group (1), remember prev', () => {
    expect(computeMaximizeRatio('thread', 0.4, 0.4, null)).toEqual({ next: 1, newPrev: 0.4 });
  });
  it('drawer focused behaves as the Threads pane group', () => {
    expect(computeMaximizeRatio('drawer', 0.3, 0.3, null)).toEqual({ next: 1, newPrev: 0.3 });
  });
  it('content focused, normal split → maximize Content pane group (0), remember prev', () => {
    expect(computeMaximizeRatio('content', 0.4, 0.4, null)).toEqual({ next: 0, newPrev: 0.4 });
  });
  it('thread focused, already maximized (1) → restore the remembered ratio', () => {
    expect(computeMaximizeRatio('thread', 1, 1, 0.4)).toEqual({ next: 0.4, newPrev: null });
  });
  it('content focused, already maximized (0) → restore the remembered ratio', () => {
    expect(computeMaximizeRatio('content', 0, 0, 0.25)).toEqual({ next: 0.25, newPrev: null });
  });
  it('already maximized with nothing remembered (e.g. after reload) → restore to default', () => {
    expect(computeMaximizeRatio('thread', 1, 1, null)).toEqual({ next: DEFAULT_SPLIT_RATIO, newPrev: null });
    expect(computeMaximizeRatio('content', 0, 0, null)).toEqual({ next: DEFAULT_SPLIT_RATIO, newPrev: null });
  });

  it('content focused while the Canvas yielded → the Canvas takes the split (0), remember the stored ratio', () => {
    expect(computeMaximizeRatio('content', 1, 0.4, null)).toEqual({ next: 0, newPrev: 0.4 });
  });
  it('thread focused while the Canvas yielded → already full, a no-op that keeps the stored ratio', () => {
    expect(computeMaximizeRatio('thread', 1, 0.4, null)).toBeNull();
    expect(computeMaximizeRatio('drawer', 1, 0.4, 0.3)).toBeNull();
  });
});
