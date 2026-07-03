import { describe, it, expect } from 'vitest';
import {
  computeSnapRatio,
  computeDrawerSnap,
  computeStepRatio,
  computeDrawerStepWidth,
  KEYBOARD_RESIZE_STEP_PX,
  MIN_THREAD_PANE_PX,
  MIN_CONTENT_PANE_PX,
} from './splitHelpers';
import { MIN_DRAWER_WIDTH } from '../../store/store';

const TOTAL = 1000;

describe('computeSnapRatio — deferred snap after a free divider drag', () => {
  it('stays put when both panes are at or above their minimums', () => {
    expect(computeSnapRatio(0.5, TOTAL)).toBeNull();
    expect(computeSnapRatio(MIN_THREAD_PANE_PX / TOTAL, TOTAL)).toBeNull();
    expect(computeSnapRatio((TOTAL - MIN_CONTENT_PANE_PX) / TOTAL, TOTAL)).toBeNull();
  });

  it('collapses the thread pane below half its minimum', () => {
    expect(computeSnapRatio((MIN_THREAD_PANE_PX / 2 - 1) / TOTAL, TOTAL)).toBe(0);
  });

  it('snaps the thread pane up to its minimum between half-min and min', () => {
    expect(computeSnapRatio((MIN_THREAD_PANE_PX / 2) / TOTAL, TOTAL)).toBe(MIN_THREAD_PANE_PX / TOTAL);
    expect(computeSnapRatio((MIN_THREAD_PANE_PX - 1) / TOTAL, TOTAL)).toBe(MIN_THREAD_PANE_PX / TOTAL);
  });

  it('collapses the content pane below half its minimum', () => {
    const ratio = (TOTAL - MIN_CONTENT_PANE_PX / 2 + 1) / TOTAL;
    expect(computeSnapRatio(ratio, TOTAL)).toBe(1);
  });

  it('snaps the content pane up to its minimum between half-min and min', () => {
    const ratio = (TOTAL - MIN_CONTENT_PANE_PX + 1) / TOTAL;
    expect(computeSnapRatio(ratio, TOTAL)).toBe((TOTAL - MIN_CONTENT_PANE_PX) / TOTAL);
  });

  it('leaves fully collapsed panes alone — release at the edge is a settled state', () => {
    expect(computeSnapRatio(0, TOTAL)).toBeNull();
    expect(computeSnapRatio(1, TOTAL)).toBeNull();
  });

  it('is a no-op while the container has no width yet', () => {
    expect(computeSnapRatio(0.5, 0)).toBeNull();
  });

  it('prefers the thread side when the window cannot fit two minimum panes', () => {
    // total < thread-min + content-min: snapping thread to its minimum
    // necessarily leaves content small.
    const total = MIN_THREAD_PANE_PX + 100;
    expect(computeSnapRatio(0.5, total)).toBe(MIN_THREAD_PANE_PX / total);
  });

  it('clamps the snap target to 1 when the container is narrower than one minimum pane', () => {
    // Drawer dragged very wide → split container < MIN. MIN / total would be
    // > 1, outside the persisted/rendered [0, 1] ratio domain.
    const total = MIN_THREAD_PANE_PX - 50;
    const ratio = (MIN_THREAD_PANE_PX / 2 + 10) / total; // above half-min, below min
    expect(computeSnapRatio(ratio, total)).toBe(1);
  });
});

describe('computeDrawerSnap — deferred snap for the thread drawer', () => {
  const MIN = 260;

  it('stays put at or above the minimum', () => {
    expect(computeDrawerSnap(MIN, MIN)).toBeNull();
    expect(computeDrawerSnap(400, MIN)).toBeNull();
  });

  it('snaps to the minimum between half-min and min', () => {
    expect(computeDrawerSnap(MIN / 2, MIN)).toBe('min');
    expect(computeDrawerSnap(MIN - 1, MIN)).toBe('min');
  });

  it('closes below half the minimum', () => {
    expect(computeDrawerSnap(MIN / 2 - 1, MIN)).toBe('close');
    expect(computeDrawerSnap(0, MIN)).toBe('close');
  });
});

describe('computeStepRatio — keyboard resize of the split divider', () => {
  const STEP = KEYBOARD_RESIZE_STEP_PX;

  it('steps the divider by the given delta in both directions', () => {
    expect(computeStepRatio(0.5, TOTAL, STEP)).toBe((500 + STEP) / TOTAL);
    expect(computeStepRatio(0.5, TOTAL, -STEP)).toBe((500 - STEP) / TOTAL);
  });

  it('clamps immediately at the thread-pane minimum instead of collapsing', () => {
    const nearMin = (MIN_THREAD_PANE_PX + 10) / TOTAL;
    expect(computeStepRatio(nearMin, TOTAL, -STEP)).toBe(MIN_THREAD_PANE_PX / TOTAL);
    // Already at the minimum: nothing to do.
    expect(computeStepRatio(MIN_THREAD_PANE_PX / TOTAL, TOTAL, -STEP)).toBeNull();
  });

  it('clamps immediately at the content-pane minimum instead of collapsing', () => {
    const ceil = (TOTAL - MIN_CONTENT_PANE_PX) / TOTAL;
    const nearCeil = (TOTAL - MIN_CONTENT_PANE_PX - 10) / TOTAL;
    expect(computeStepRatio(nearCeil, TOTAL, STEP)).toBe(ceil);
    expect(computeStepRatio(ceil, TOTAL, STEP)).toBeNull();
  });

  it('reopens a collapsed pane at its minimum when stepping back in', () => {
    // Thread pane collapsed; widening reopens it at MIN_THREAD_PANE_PX.
    expect(computeStepRatio(0, TOTAL, STEP)).toBe(MIN_THREAD_PANE_PX / TOTAL);
    // Content pane collapsed; narrowing the thread pane reopens content at its minimum.
    expect(computeStepRatio(1, TOTAL, -STEP)).toBe((TOTAL - MIN_CONTENT_PANE_PX) / TOTAL);
  });

  it('treats a collapsed pane as settled against steps pushing further out', () => {
    expect(computeStepRatio(0, TOTAL, -STEP)).toBeNull();
    expect(computeStepRatio(1, TOTAL, STEP)).toBeNull();
  });

  it('is a no-op while the container has no width or cannot fit both minimums', () => {
    expect(computeStepRatio(0.5, 0, STEP)).toBeNull();
    expect(computeStepRatio(0.5, MIN_THREAD_PANE_PX + MIN_CONTENT_PANE_PX - 1, STEP)).toBeNull();
  });
});

describe('computeDrawerStepWidth — keyboard resize of the thread drawer', () => {
  const STEP = KEYBOARD_RESIZE_STEP_PX;
  const MAX = 900;

  it('steps the width by the given delta in both directions', () => {
    expect(computeDrawerStepWidth(400, STEP, MAX)).toBe(400 + STEP);
    expect(computeDrawerStepWidth(400, -STEP, MAX)).toBe(400 - STEP);
  });

  it('clamps at the drawer minimum instead of closing', () => {
    expect(computeDrawerStepWidth(MIN_DRAWER_WIDTH + 10, -STEP, MAX)).toBe(MIN_DRAWER_WIDTH);
    expect(computeDrawerStepWidth(MIN_DRAWER_WIDTH, -STEP, MAX)).toBeNull();
  });

  it('clamps at maxPx', () => {
    expect(computeDrawerStepWidth(MAX - 10, STEP, MAX)).toBe(MAX);
    expect(computeDrawerStepWidth(MAX, STEP, MAX)).toBeNull();
  });

  it('is a no-op when the row cannot host even a minimum-width drawer', () => {
    expect(computeDrawerStepWidth(300, STEP, MIN_DRAWER_WIDTH - 1)).toBeNull();
  });
});
