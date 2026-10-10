// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { nearestScrollableAncestor, revealScrollTop } from './focusReveal';

/** A 600px-tall container showing content from y=100 to y=700, scrolled to 0.
 *  Roughly the shape of `.content-pane-body` under an open mobile keyboard.
 *  Equal margins here; the asymmetric block at the end covers the accessory
 *  strip, which only the bottom edge owes. */
const view = { scrollTop: 0, viewTop: 100, viewBottom: 700, marginTop: 16, marginBottom: 16 };

describe('revealScrollTop', () => {
  it('lifts a field hidden below the fold, leaving the margin clear', () => {
    // Field at 720..760: 60px below the fold once the margin is counted.
    const next = revealScrollTop({ ...view, fieldTop: 720, fieldBottom: 760 });
    expect(next).toBe(76);
  });

  it('leaves a comfortably visible field alone', () => {
    expect(revealScrollTop({ ...view, fieldTop: 300, fieldBottom: 340 })).toBeNull();
  });

  it('treats a sub-pixel move as nothing owed', () => {
    // Bottom edge 0.4px past the margin, which a real layout produces often.
    expect(revealScrollTop({ ...view, fieldTop: 644, fieldBottom: 684.4 })).toBeNull();
  });

  it('adds the move to the container position it was given', () => {
    const next = revealScrollTop({ ...view, scrollTop: 250, fieldTop: 720, fieldBottom: 760 });
    expect(next).toBe(326);
  });

  it('shows the TOP of a field taller than the view', () => {
    // Field 800px tall in a 600px view. The bottom cannot be reached without
    // pushing the first line off the top, so the top wins.
    const next = revealScrollTop({ ...view, fieldTop: 500, fieldBottom: 1300 });
    expect(next).toBe(384);
    // The top lands exactly on the margin, never above it.
    expect(500 - (next ?? 0)).toBe(view.viewTop + view.marginTop);
  });

  it('does not move a tall field that already fills the view', () => {
    expect(revealScrollTop({ ...view, fieldTop: 50, fieldBottom: 850 })).toBeNull();
  });

  it('pulls a field back down when it sits above the view', () => {
    const next = revealScrollTop({ ...view, scrollTop: 400, fieldTop: 40, fieldBottom: 80 });
    expect(next).toBe(324);
    expect(40 - ((next ?? 0) - 400)).toBe(view.viewTop + view.marginTop);
  });

  it('gives a tall field its top even when its bottom is inside the view', () => {
    // Top 200px above the view, bottom 20px inside it: 780px of field in a
    // 600px view, so no offset satisfies both margins. One rule decides every
    // such case, and it is the same one the case above states: the top wins.
    const next = revealScrollTop({ ...view, scrollTop: 400, fieldTop: -100, fieldBottom: 680 });
    expect(next).toBe(184);
    expect(-100 - (184 - 400)).toBe(view.viewTop + view.marginTop);
  });

  it('never asks for a negative offset', () => {
    // A tall field already covering the view wants to scroll up past the top.
    expect(revealScrollTop({ ...view, fieldTop: -400, fieldBottom: 900 })).toBeNull();
  });

  // iOS floats its keyboard accessory bar over the foot of the visual
  // viewport, so the bottom edge owes a strip the top edge does not.
  const band = { ...view, marginBottom: 80 };

  it('clears the whole accessory strip below the field', () => {
    // Field at 640..680 is inside the view, and under the floating bar.
    expect(revealScrollTop({ ...band, fieldTop: 640, fieldBottom: 680 })).toBe(60);
  });

  it('does not charge the strip to the top edge', () => {
    // Pulled down to `marginTop`, not to the larger bottom margin.
    const next = revealScrollTop({ ...band, scrollTop: 400, fieldTop: 40, fieldBottom: 80 });
    expect(next).toBe(324);
  });
});

describe('nearestScrollableAncestor', () => {
  /** Give jsdom the layout numbers it has no engine to compute. */
  function sized(el: HTMLElement, scrollHeight: number, clientHeight: number) {
    Object.defineProperty(el, 'scrollHeight', { value: scrollHeight, configurable: true });
    Object.defineProperty(el, 'clientHeight', { value: clientHeight, configurable: true });
    return el;
  }

  function tree(overflowY: string, scrollHeight: number) {
    const pane = document.createElement('div');
    pane.style.overflowY = overflowY;
    sized(pane, scrollHeight, 600);
    const group = pane.appendChild(document.createElement('div'));
    const field = group.appendChild(document.createElement('input'));
    document.body.appendChild(pane);
    return { pane, field };
  }

  it('finds the scrolling ancestor through intermediate elements', () => {
    const { pane, field } = tree('auto', 1400);
    expect(nearestScrollableAncestor(field)).toBe(pane);
  });

  it('accepts overflow-y: scroll', () => {
    const { pane, field } = tree('scroll', 1400);
    expect(nearestScrollableAncestor(field)).toBe(pane);
  });

  it('skips an ancestor that declares scrolling but does not overflow', () => {
    const { field } = tree('auto', 600);
    expect(nearestScrollableAncestor(field)).toBeNull();
  });

  it('skips an ancestor that overflows but clips instead of scrolling', () => {
    const { field } = tree('hidden', 1400);
    expect(nearestScrollableAncestor(field)).toBeNull();
  });

  it('answers null for a detached or parentless element', () => {
    expect(nearestScrollableAncestor(null)).toBeNull();
    expect(nearestScrollableAncestor(document.createElement('input'))).toBeNull();
  });
});
