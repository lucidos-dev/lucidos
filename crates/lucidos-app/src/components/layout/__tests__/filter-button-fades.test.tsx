// @vitest-environment jsdom
/**
 * The Filter button's glyph fades rather than swaps. It keeps every glyph it
 * can show MOUNTED and flips which one is shown, so the CSS transition runs on
 * nodes that already exist. Under Ongoing the whole button fades out in a slot
 * that keeps its box. The pane title switches word at once, and fades out only
 * when its word no longer fits.
 *
 * These tests pin the DOM the CSS keys on. The frames are covered by
 * `e2e/threads-header-filter-transitions.spec.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FilterButtonGlyph, ThreadsPaneTitle, titleFits, groupingButtonBadgeCount, groupingButtonLabel, otherGrouping } from '../ThreadsHeaderControls';
import { FILTER_BUTTON_GLYPHS, type FilterGlyph } from '../ThreadFilterPanel';
import { openThreadFilterPanel, closeThreadFilterPanel } from '../../../store/threadFilterPanel';
import { setDrawerGrouping } from '../../../store/store';

let host: HTMLElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
});

const layers = () => Array.from(host.querySelectorAll<HTMLElement>('.crossfade-layer'));
const current = () => layers().filter(l => l.hasAttribute('data-current'));

describe('the Filter glyph crossfades', () => {
  const show = (glyph: FilterGlyph) =>
    act(() => { render(<FilterButtonGlyph glyph={glyph} />, host); });

  it('keeps every glyph mounted, with exactly the current one shown', () => {
    show('all');
    expect(layers().map(l => l.dataset.layer)).toEqual(Object.keys(FILTER_BUTTON_GLYPHS));
    expect(current()).toHaveLength(1);
    // The hidden ones are decoration only.
    for (const l of layers()) {
      expect(l.getAttribute('aria-hidden')).toBe(l.hasAttribute('data-current') ? null : 'true');
    }
  });

  // Outline to solid and back: the same nodes with the shown flag moved, which
  // is what lets the opacity transition run and reverse.
  it('moves the shown flag between the same nodes on every change', () => {
    show('all');
    const before = layers();
    for (const glyph of ['filtered', 'all', 'filtered', 'all'] as const) {
      show(glyph);
      expect(layers()).toEqual(before);
      expect(current().map(l => l.dataset.layer)).toEqual([glyph]);
    }
  });

  it('wraps the stack in the element that carries the translucency', () => {
    show('filtered');
    expect(host.firstElementChild?.classList.contains('crossfade-glyph')).toBe(true);
  });
});

describe('the pane title switches word at once', () => {
  const show = (filters: boolean) => act(() => {
    if (filters) openThreadFilterPanel(); else closeThreadFilterPanel();
    render(<ThreadsPaneTitle class="threads-header-title" />, host);
  });
  const title = () => host.firstElementChild as HTMLElement;

  beforeEach(() => vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} }));
  afterEach(() => {
    act(() => { render(null, host); });
    closeThreadFilterPanel();
    setDrawerGrouping('folders');
    vi.unstubAllGlobals();
  });

  it('names the grouping, or the filter panel, in one plain element', () => {
    act(() => setDrawerGrouping('folders'));
    show(false);
    expect(title().textContent).toBe('Folders');
    expect(title().className).toBe('threads-header-title');
    expect(layers()).toHaveLength(0);
    show(true);
    expect(title().textContent).toBe('Filters');
    show(false);
    act(() => setDrawerGrouping('ongoing'));
    expect(title().textContent).toBe('Ongoing');
  });

  it('is whole or hidden: a word that outgrows its box is marked cramped', () => {
    expect(titleFits(60, 60)).toBe(true);
    expect(titleFits(60.4, 60)).toBe(true);
    expect(titleFits(61, 60)).toBe(false);
    expect(titleFits(60, 0)).toBe(false);
  });
});

describe('the grouping button', () => {
  it('offers the other grouping', () => {
    expect(otherGrouping('folders')).toBe('ongoing');
    expect(otherGrouping('ongoing')).toBe('folders');
  });

  it('badges the attention count under Folders only', () => {
    // Under Ongoing the Blocked group's own header carries it.
    expect(groupingButtonBadgeCount('folders', 3)).toBe(3);
    expect(groupingButtonBadgeCount('ongoing', 3)).toBe(0);
  });

  it('names what a press does, and carries the badge count for assistive tech', () => {
    expect(groupingButtonLabel('folders', 0)).toBe('Show Ongoing');
    expect(groupingButtonLabel('folders', 2)).toBe('Show Ongoing (2 blocked)');
    expect(groupingButtonLabel('ongoing', 0)).toBe('Show Folders');
  });
});
