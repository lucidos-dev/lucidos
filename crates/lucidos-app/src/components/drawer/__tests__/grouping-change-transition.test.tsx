// @vitest-environment jsdom
/**
 * Every grouping change plays the same transition, whoever made it.
 *
 * The drawer's navigation cover keys on `drawerSwapKey(panelOpen, grouping)`,
 * so a change of either half is one dip. With the panel shut, a grouping
 * change leaves a drawing of the old list over the list. The CSS hides it at
 * the dip's midpoint, as it hides the closing panel.
 *
 * The filter shapes Folders only: switching to Ongoing closes the panel and
 * hides the Filter button. jsdom runs no animations and loads no CSS, so these
 * tests pin the DOM the CSS keys on. The frames are covered by
 * `e2e/drawer-grouping.spec.ts`.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer } from '../ThreadDrawer';
import { drawerSwapKey } from '../ThreadFilterCover';
import { drawLeavingList } from '../LeavingViewDrawing';
import { ThreadFilterButton, ThreadGroupingButton } from '../../layout/ThreadsHeaderControls';
import { drawerGrouping, setDrawerGrouping, threadsLoaded } from '../../../store/store';
import { openThreadFilterPanel, closeThreadFilterPanel, threadFilterPanelOpen } from '../../../store/threadFilterPanel';
import '../../../store/effects';

vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

describe('drawerSwapKey', () => {
  it('names the panel while it is open, whatever the grouping', () => {
    expect(drawerSwapKey(true, 'folders')).toBe('filters');
    expect(drawerSwapKey(true, 'ongoing')).toBe('filters');
  });

  it('names the grouping while the panel is shut, so each change is a new key', () => {
    expect(drawerSwapKey(false, 'folders')).toBe('threads:folders');
    expect(drawerSwapKey(false, 'ongoing')).not.toBe(drawerSwapKey(false, 'folders'));
  });
});

describe('a grouping change', () => {
  let host: HTMLElement;
  const q = (s: string) => host.querySelector<HTMLElement>(s);
  const navCovers = () => host.querySelectorAll('.thread-drawer-body > .nav-cover');
  const groupingButton = () => q('.grouping-btn') as HTMLButtonElement;
  const shownGlyph = () => groupingButton().querySelector<HTMLElement>('.crossfade-layer[data-current]')!.dataset.layer;
  /** Runs every fuse out, so the next change starts a fresh dip. */
  const settle = () => act(() => { vi.advanceTimersByTime(5_000); });

  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    closeThreadFilterPanel();
    setDrawerGrouping('folders');
    threadsLoaded.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => {
      render(<><ThreadFilterButton /><ThreadGroupingButton /><ThreadDrawer forceVisible /></>, host);
    });
  });

  afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    closeThreadFilterPanel();
    setDrawerGrouping('folders');
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('starts on Folders, with the button offering Ongoing and no cover', () => {
    expect(groupingButton().getAttribute('aria-label')).toBe('Show Ongoing');
    expect(shownGlyph()).toBe('ongoing');
    expect(navCovers()).toHaveLength(0);
  });

  it('swaps the grouping on each press, and the glyph shows where the next goes', () => {
    act(() => { groupingButton().click(); });
    expect(drawerGrouping.value).toBe('ongoing');
    expect(groupingButton().getAttribute('aria-label')).toBe('Show Folders');
    expect(shownGlyph()).toBe('folders');
    settle();
    act(() => { groupingButton().click(); });
    expect(drawerGrouping.value).toBe('folders');
    expect(shownGlyph()).toBe('ongoing');
  });

  it('plays one fresh dip from the button and from the store alike', () => {
    act(() => { groupingButton().click(); });
    const byButton = { grouping: drawerGrouping.value, dips: navCovers().length === 1 };
    settle();
    act(() => { setDrawerGrouping('folders'); });
    settle();
    act(() => { setDrawerGrouping('ongoing'); });
    const byStore = { grouping: drawerGrouping.value, dips: navCovers().length === 1 };
    expect(byButton).toEqual({ grouping: 'ongoing', dips: true });
    expect(byStore).toEqual(byButton);
  });

  it('holds a drawing of the leaving list over the list, inert and with no identity', () => {
    act(() => { groupingButton().click(); });
    const drawing = q('.thread-drawer-body > .thread-view-drawing');
    expect(drawing).not.toBeNull();
    expect(drawing!.getAttribute('aria-hidden')).toBe('true');
    const drawn = drawing!.firstElementChild as HTMLElement;
    expect(drawn.className).toBe('thread-view-drawing-list');
    expect(drawn.inert).toBe(true);
    expect(drawn.querySelector('[id], [data-thread-nav], [data-flip-id]')).toBeNull();
    // The real list already shows the ongoing groups under it.
    expect(q('.thread-drawer-body > .thread-drawer-list .ongoing-grouping')).not.toBeNull();
    // Painted under the filter cover and the navigation cover.
    expect(drawing!.nextElementSibling!.classList.contains('thread-filter-cover')).toBe(true);
  });

  it('drops the drawing on the cover fuse', () => {
    act(() => { groupingButton().click(); });
    expect(q('.thread-view-drawing')).not.toBeNull();
    settle();
    expect(q('.thread-view-drawing')).toBeNull();
    expect(navCovers()).toHaveLength(0);
  });

  it('closes the filter panel and hides the Filter button under Ongoing', () => {
    act(() => { openThreadFilterPanel(); });
    expect(threadFilterPanelOpen.value).toBe(true);
    act(() => { setDrawerGrouping('ongoing'); });
    expect(threadFilterPanelOpen.value).toBe(false);
    const slot = q('.filter-slot')!;
    expect(slot.hasAttribute('data-shown')).toBe(false);
    expect(slot.hasAttribute('inert')).toBe(true);
    // An open under Ongoing does nothing.
    act(() => { openThreadFilterPanel(); });
    expect(threadFilterPanelOpen.value).toBe(false);
    act(() => { setDrawerGrouping('folders'); });
    expect(q('.filter-slot')!.hasAttribute('data-shown')).toBe(true);
  });

  it('draws no grouping band in the drawer: the header owns the pick', () => {
    expect(q('.thread-drawer [role="radiogroup"]')).toBeNull();
    expect(q('.thread-drawer > :not(.thread-drawer-body)')).toBeNull();
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
