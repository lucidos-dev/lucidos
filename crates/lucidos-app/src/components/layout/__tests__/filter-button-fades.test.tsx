// @vitest-environment jsdom
/**
 * The Filter button's glyph and badge, and the pane title, fade rather than
 * swap. Each keeps every state it can show MOUNTED and flips which one is
 * shown, so the CSS transition runs on nodes that already exist.
 *
 * These tests pin that shape: the same nodes survive a state change, and the
 * attribute the CSS keys on moves. The frames are covered by
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

describe('the pane title crossfades between Threads and Filters', () => {
  const show = (filters: boolean) => act(() => {
    if (filters) openThreadFilterPanel(); else closeThreadFilterPanel();
    render(<ThreadsPaneTitle class="threads-header-title" />, host);
  });

  it('keeps both words mounted and moves the shown flag', () => {
    show(false);
    const before = layers();
    expect(before.map(l => l.textContent)).toEqual(['Threads', 'Filters']);
    expect(current().map(l => l.textContent)).toEqual(['Threads']);
    show(true);
    expect(layers()).toEqual(before);
    expect(current().map(l => l.textContent)).toEqual(['Filters']);
    expect(before[0].getAttribute('aria-hidden')).toBe('true');
    show(false);
    expect(current().map(l => l.textContent)).toEqual(['Threads']);
  });

  it('puts the title class on the stack itself, so the header centring still applies', () => {
    show(false);
    const root = host.firstElementChild!;
    expect(root.classList.contains('threads-header-title')).toBe(true);
    expect(root.classList.contains('crossfade-stack')).toBe(true);
  });
});
