// @vitest-environment jsdom
/**
 * Every status filter change plays the same transition, whoever made it.
 *
 * The drawer's navigation cover keys on `drawerSwapKey(panelOpen, view)`, so a
 * change of either half is one dip. With the panel shut, a status change
 * leaves a drawing of the old list over the pane. The CSS hides it at the
 * dip's midpoint, as it hides the closing panel. The Filter button's glyph
 * keys on the same value, so it crossfades the same way on every path.
 *
 * jsdom runs no animations and loads no CSS, so these tests pin the DOM the
 * CSS keys on. The frames are covered by
 * `e2e/threads-header-filter-transitions.spec.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer } from '../ThreadDrawer';
import { drawerSwapKey } from '../ThreadFilterCover';
import { drawLeavingList } from '../LeavingViewDrawing';
import { ThreadFilterButton } from '../../layout/ThreadFilterButton';
import { drawerView, setDrawerView, threadsLoaded } from '../../../store/store';
import { openThreadFilterPanel, closeThreadFilterPanel } from '../../../store/threadFilterPanel';

vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

describe('drawerSwapKey', () => {
  it('names the panel while it is open, whatever the status', () => {
    expect(drawerSwapKey(true, 'all')).toBe('filters');
    expect(drawerSwapKey(true, 'review')).toBe('filters');
  });

  it('names the status while the panel is shut, so each status change is a new key', () => {
    expect(drawerSwapKey(false, 'all')).toBe('threads:all');
    expect(drawerSwapKey(false, 'attention')).not.toBe(drawerSwapKey(false, 'all'));
  });
});

describe('the link and the panel give a status change the same transition', () => {
  let host: HTMLElement;
  const q = (s: string) => host.querySelector<HTMLElement>(s);
  const navCovers = () => host.querySelectorAll('.thread-drawer > .nav-cover');
  const glyph = () => q('.filter-glyph [data-current]')?.getAttribute('data-layer');
  const seeAll = () => Array.from(host.querySelectorAll<HTMLButtonElement>('.thread-drawer-list button'))
    .find(b => b.textContent === 'See all statuses');
  const allStatusesRow = () => Array.from(host.querySelectorAll<HTMLElement>('.thread-filter-panel [role="radio"]'))
    .find(r => (r.textContent ?? '').startsWith('All statuses'));
  /** Runs every fuse out, so the next change starts a fresh dip. */
  const settle = () => act(() => { vi.advanceTimersByTime(5_000); });

  /** What one path leaves behind straight after it changes the status. */
  function transition() {
    const covers = navCovers();
    return {
      view: drawerView.value,
      dips: covers.length === 1 && covers[0].classList.contains('nav-cover-dip'),
      glyph: glyph(),
    };
  }

  beforeEach(() => {
    vi.useFakeTimers();
    // The All list's pager observes a sentinel. jsdom has no observer.
    vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
    closeThreadFilterPanel();
    setDrawerView('attention');
    threadsLoaded.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => {
      render(<><ThreadFilterButton /><ThreadDrawer forceVisible /></>, host);
    });
  });

  afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    closeThreadFilterPanel();
    setDrawerView('all');
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('starts on the empty attention view, glyph on its status, with no cover', () => {
    expect(seeAll()).toBeDefined();
    expect(glyph()).toBe('attention');
    expect(navCovers()).toHaveLength(0);
  });

  it('matches across the two paths: one fresh dip, and the glyph lands on the funnel', () => {
    act(() => { seeAll()!.click(); });
    const byLink = transition();
    settle();

    act(() => { setDrawerView('attention'); });
    settle();
    act(() => { openThreadFilterPanel(); });
    settle();
    act(() => { allStatusesRow()!.click(); });
    const byPanel = transition();

    expect(byLink).toEqual({ view: 'all', dips: true, glyph: 'all' });
    expect(byPanel).toEqual(byLink);
  });

  it('holds a drawing of the leaving list on the link path, inert and with no identity', () => {
    act(() => { seeAll()!.click(); });
    const drawing = q('.thread-drawer > .thread-view-drawing');
    expect(drawing).not.toBeNull();
    // The frame takes the tap; only the drawn content is inert.
    expect(drawing!.hasAttribute('inert')).toBe(false);
    expect(drawing!.getAttribute('aria-hidden')).toBe('true');
    const drawn = drawing!.firstElementChild as HTMLElement;
    expect(drawn.className).toBe('thread-view-drawing-list');
    expect(drawn.inert).toBe(true);
    expect(drawn.textContent).toContain('Nothing needs attention');
    expect(drawn.querySelector('[id], [data-thread-nav], [data-flip-id]')).toBeNull();
    // The real list already shows the arriving view under it.
    expect(q('.thread-drawer > .thread-drawer-list')!.textContent).not.toContain('Nothing needs attention');
    // Painted under the filter cover and the navigation cover.
    expect(drawing!.nextElementSibling!.classList.contains('thread-filter-cover')).toBe(true);
  });

  it('drops the drawing on the cover fuse', () => {
    act(() => { seeAll()!.click(); });
    expect(q('.thread-view-drawing')).not.toBeNull();
    settle();
    expect(q('.thread-view-drawing')).toBeNull();
    expect(navCovers()).toHaveLength(0);
  });

  it('carries a drawing still on screen over to the next change, as a fresh element', () => {
    act(() => { seeAll()!.click(); });
    const first = q('.thread-view-drawing');
    const drawn = first!.firstElementChild;
    act(() => { setDrawerView('review'); });
    const second = q('.thread-view-drawing');
    expect(second).not.toBe(first);
    expect(second!.firstElementChild).toBe(drawn);
    expect(host.querySelectorAll('.thread-view-drawing')).toHaveLength(1);
  });

  it('carries a drawing still on screen through a panel open, which changes no status', () => {
    act(() => { seeAll()!.click(); });
    const first = q('.thread-view-drawing');
    const drawn = first!.firstElementChild;
    act(() => { openThreadFilterPanel(); });
    const second = q('.thread-view-drawing');
    expect(second).not.toBe(first);
    expect(second!.firstElementChild).toBe(drawn);
  });
});

describe('drawLeavingList', () => {
  it('draws nothing when the list is not on screen, as under the open panel', () => {
    const pane = document.createElement('div');
    const list = document.createElement('div');
    list.style.visibility = 'hidden';
    list.appendChild(document.createElement('div'));
    pane.appendChild(list);
    document.body.appendChild(pane);
    expect(drawLeavingList(list)).toBeNull();
    pane.remove();
  });

  it('draws nothing for an empty list, as on a collapsed drawer', () => {
    expect(drawLeavingList(document.createElement('div'))).toBeNull();
  });
});
