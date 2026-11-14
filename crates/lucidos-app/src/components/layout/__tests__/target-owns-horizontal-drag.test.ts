// @vitest-environment jsdom
/**
 * A touch whose target drives a horizontal gesture of its own never starts a
 * pane swipe. The dashboard panel sits inside the thread pane, so a tile drag
 * in edit mode reaches the strip's touch listeners too.
 */
import { afterEach, describe, expect, it } from 'vitest';
import { targetOwnsHorizontalDrag } from '../MobileSwipeContainer';

function mount(html: string): Element {
  document.body.innerHTML = html;
  return document.querySelector('[data-target]')!;
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('targetOwnsHorizontalDrag', () => {
  it('leaves an ordinary touch to the pane swipe', () => {
    expect(targetOwnsHorizontalDrag(mount('<div><p data-target>text</p></div>'))).toBe(false);
  });

  it('claims a range slider', () => {
    expect(targetOwnsHorizontalDrag(mount('<input type="range" data-target>'))).toBe(true);
  });

  it('claims a dashboard tile being edited, under its drag shield', () => {
    const target = mount('<div class="dashboard-tile"><div class="dashboard-tile-shield"><span data-target></span></div></div>');
    expect(targetOwnsHorizontalDrag(target)).toBe(true);
  });

  it('leaves a dashboard tile outside edit mode to the pane swipe', () => {
    expect(targetOwnsHorizontalDrag(mount('<div class="dashboard-tile"><span data-target></span></div>'))).toBe(false);
  });
});
