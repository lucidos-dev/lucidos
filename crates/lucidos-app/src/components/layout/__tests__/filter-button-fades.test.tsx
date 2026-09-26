// @vitest-environment jsdom
/**
 * The Filter button's glyph and badge fade rather than swap. Each keeps every
 * state it can show MOUNTED and flips which one is shown, so the CSS transition
 * runs on nodes that already exist. The pane title is the other shape: it
 * arrives with the drawer's navigation cover, as a fresh keyed element.
 *
 * These tests pin both shapes. The frames are covered by
 * `e2e/threads-header-filter-transitions.spec.ts`.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FilterButtonGlyph, FilterButtonBadge, ThreadsPaneTitle } from '../ThreadFilterButton';
import { FILTER_BUTTON_GLYPHS, type FilterGlyph } from '../ThreadFilterPanel';
import { openThreadFilterPanel, closeThreadFilterPanel } from '../../../store/threadFilterPanel';

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

  // Outline to solid and back, funnel to status, status to status and back:
  // each is the same nodes with the shown flag moved, which is what lets the
  // opacity transition run and reverse.
  it('moves the shown flag between the same nodes on every change', () => {
    show('all');
    const before = layers();
    for (const glyph of ['filtered', 'all', 'review', 'running', 'all'] as const) {
      show(glyph);
      expect(layers()).toEqual(before);
      expect(current().map(l => l.dataset.layer)).toEqual([glyph]);
    }
  });

  it('wraps the stack in the element that carries the translucency', () => {
    show('filtered');
    expect(host.firstElementChild?.classList.contains('filter-glyph')).toBe(true);
  });
});

describe('the needs-attention badge fades both ways', () => {
  const badge = () => host.querySelector('.badge.filter-badge') as HTMLElement;
  const show = (count: number) => act(() => { render(<FilterButtonBadge count={count} />, host); });

  it('is mounted but not shown while there is nothing to count', () => {
    show(0);
    expect(badge()).not.toBeNull();
    expect(badge().hasAttribute('data-shown')).toBe(false);
    expect(badge().textContent).toBe('');
  });

  it('appears, changes count and disappears on one node', () => {
    show(0);
    const node = badge();
    show(2);
    expect(badge()).toBe(node);
    expect(node.hasAttribute('data-shown')).toBe(true);
    expect(node.textContent).toBe('2');
    // A count change while shown only repaints the number.
    show(3);
    expect(badge()).toBe(node);
    expect(node.hasAttribute('data-shown')).toBe(true);
    expect(node.textContent).toBe('3');
    // Going to 0 (or the panel opening) fades it out still reading 3.
    show(0);
    expect(badge()).toBe(node);
    expect(node.hasAttribute('data-shown')).toBe(false);
    expect(node.textContent).toBe('3');
  });

  it('stays out of the accessibility tree, since the button names itself', () => {
    show(4);
    expect(badge().getAttribute('aria-hidden')).toBe('true');
  });
});

describe('the pane title arrives with the drawer view', () => {
  const show = (filters: boolean) => act(() => {
    if (filters) openThreadFilterPanel(); else closeThreadFilterPanel();
    render(<ThreadsPaneTitle class="threads-header-title" />, host);
  });
  const title = () => host.firstElementChild as HTMLElement;

  afterEach(() => closeThreadFilterPanel());

  it('says what the pane shows, and does not fade on the first render', () => {
    show(false);
    expect(title().textContent).toBe('Threads');
    expect(title().classList.contains('threads-header-title')).toBe(true);
    expect(title().classList.contains('nav-arrive')).toBe(false);
  });

  it('arrives as a fresh element on each swap, so its fade replays', () => {
    show(false);
    const threads = title();
    show(true);
    expect(title()).not.toBe(threads);
    expect(title().textContent).toBe('Filters');
    expect(title().classList.contains('nav-arrive')).toBe(true);
    // The header centring still applies to the arriving word.
    expect(title().classList.contains('threads-header-title')).toBe(true);
    const filters = title();
    show(false);
    expect(title()).not.toBe(filters);
    expect(title().textContent).toBe('Threads');
    expect(title().classList.contains('nav-arrive')).toBe(true);
  });
});
