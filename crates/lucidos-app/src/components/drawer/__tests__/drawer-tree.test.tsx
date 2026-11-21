// @vitest-environment jsdom
/**
 * The drawer's tree, `.thread-drawer-tree`:
 * - ARIA 1.2 lets a tree own only tree items and groups. The grouping is
 *   picked in the header. So under both groupings and on both layouts, the
 *   drawer holds one tree with every tree item inside it.
 * - Focus that lands on the scroller around the tree moves on to the tree,
 *   which holds the list nav.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer } from '../ThreadDrawer';
import {
    setDrawerGrouping, setSelectedOngoingGroup, threadMap, threadsLoaded,
    threadSearchQuery, threadSearchResults, setThreadChannelFilter, ALL_CHANNELS,
    type DrawerGrouping,
} from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { _resetComposeDraftsForTesting } from '../../../store/composeDrafts';
import { viewportIsMobile } from '../../../utils/viewport';

vi.mock('../../../store/actions/thread-loading', () => ({
    loadThreadEvents: vi.fn(),
    loadOlderThreads: vi.fn(),
    reloadAfterFilterChange: vi.fn(),
    filterChangedSinceLoad: () => false,
    ensureThreadInMap: vi.fn(),
}));

let host: HTMLDivElement;
const wasMobile = viewportIsMobile.value;

beforeEach(() => {
    vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    _resetComposeDraftsForTesting();
    setThreadChannelFilter(new Set(ALL_CHANNELS));
    threadSearchQuery.value = '';
    threadSearchResults.value = { status: 'not-loaded' };
    threadMap.value = new Map([
        ['q', makeThreadState('q', { meta: { section: 'inbox', status: 'waiting_for_user_answer', title: 'Question', createdAt: '2026-01-02T00:00:00Z' } })],
        ['idle', makeThreadState('idle', { meta: { section: 'inbox', title: 'Idle one', createdAt: '2026-01-01T00:00:00Z' } })],
    ]);
    threadsLoaded.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
});

afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    viewportIsMobile.value = wasMobile;
    setDrawerGrouping('folders');
    setSelectedOngoingGroup('blocked');
    vi.unstubAllGlobals();
});

const cases: [string, boolean, DrawerGrouping][] = [
    ['desktop', false, 'folders'],
    ['desktop', false, 'ongoing'],
    ['phone', true, 'folders'],
    ['phone', true, 'ongoing'],
];

describe('the drawer tree', () => {
    it.each(cases)('%s, %s: one tree, every tree item inside it', (_layout, mobile, grouping) => {
        viewportIsMobile.value = mobile;
        setDrawerGrouping(grouping);
        act(() => { render(<ThreadDrawer forceVisible />, host); });

        expect(host.querySelectorAll('[role="tree"]')).toHaveLength(1);
        expect(host.querySelector('[role="radiogroup"]')).toBeNull();

        const items = Array.from(host.querySelectorAll('[role="treeitem"]'));
        expect(items.length).toBeGreaterThan(0);
        for (const item of items) expect(item.closest('[role="tree"]')).not.toBeNull();
    });
});

describe('the scroller around the tree', () => {
    it('hands its focus to the tree, as a click on blank list space gives it', () => {
        viewportIsMobile.value = false;
        act(() => { render(<ThreadDrawer forceVisible />, host); });
        act(() => { host.querySelector<HTMLElement>('.thread-drawer-list')!.focus(); });
        expect(document.activeElement).toBe(host.querySelector('.thread-drawer-tree'));
    });
});
