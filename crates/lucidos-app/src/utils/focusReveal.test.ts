// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { nearestScrollableAncestor, revealScrollTop } from './focusReveal';

const view = { scrollTop: 0, viewTop: 100, viewBottom: 700, marginTop: 16, marginBottom: 16 };

describe('revealScrollTop', () => {
  it('lifts a field hidden below the fold, leaving the margin clear', () => {
    const next = revealScrollTop({ ...view, fieldTop: 720, fieldBottom: 760 });
    expect(next).toBe(76);
  });

  it('leaves a comfortably visible field alone', () => {
    expect(revealScrollTop({ ...view, fieldTop: 300, fieldBottom: 340 })).toBeNull();
  });

  it('treats a sub-pixel move as nothing owed', () => {
    expect(revealScrollTop({ ...view, fieldTop: 644, fieldBottom: 684.4 })).toBeNull();
  });

  it('adds the move to the container position it was given', () => {
    const next = revealScrollTop({ ...view, scrollTop: 250, fieldTop: 720, fieldBottom: 760 });
    expect(next).toBe(326);
  });

  it('shows the TOP of a field taller than the view', () => {
    const next = revealScrollTop({ ...view, fieldTop: 500, fieldBottom: 1300 });
    expect(next).toBe(384);
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
    const next = revealScrollTop({ ...view, scrollTop: 400, fieldTop: -100, fieldBottom: 680 });
    expect(next).toBe(184);
    expect(-100 - (184 - 400)).toBe(view.viewTop + view.marginTop);
  });

  it('never asks for a negative offset', () => {
    expect(revealScrollTop({ ...view, fieldTop: -400, fieldBottom: 900 })).toBeNull();
  });

  const band = { ...view, marginBottom: 80 };

  it('clears the whole accessory strip below the field', () => {
    expect(revealScrollTop({ ...band, fieldTop: 640, fieldBottom: 680 })).toBe(60);
  });

  it('does not charge the strip to the top edge', () => {
    const next = revealScrollTop({ ...band, scrollTop: 400, fieldTop: 40, fieldBottom: 80 });
    expect(next).toBe(324);
  });
});

describe('nearestScrollableAncestor', () => {
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
