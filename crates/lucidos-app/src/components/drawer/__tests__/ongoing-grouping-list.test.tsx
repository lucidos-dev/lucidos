// @vitest-environment jsdom
/**
 * The Ongoing grouping as the drawer renders it:
 * - four tiles, always, in one fixed order, whatever their counts;
 * - exactly one selected, and only its threads listed below the tiles;
 * - a press opens the group's first thread, on the phone only when it is the sole one;
 * - a press on the selected In flight tile lists only running, then only waiting, then all;
 * - a selected group that is empty says so, and nothing is claimed before load;
 * - every thread type, whatever the thread-type filter says;
 * - no Idle group, and no Archive all.
 * The header's grouping button carries the Blocked count under
 * Folders, where it offers Ongoing.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer, moveHighlight, selectHighlighted, expandHighlighted, collapseHighlighted } from '../ThreadDrawer';
import {
    setDrawerGrouping, setSelectedOngoingGroup, selectedOngoingGroup, threadMap, threadsLoaded, focusedThreadId,
    threadSearchQuery, threadSearchResults, setThreadChannelFilter, ALL_CHANNELS,
} from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { _resetComposeDraftsForTesting, patchDraft } from '../../../store/composeDrafts';
import { handleArchiveAll } from '../../../store/actions/archive-all';
import { ThreadGroupingButton } from '../../layout/ThreadsHeaderControls';

vi.mock('../../../store/actions/thread-loading', () => ({
    loadThreadEvents: vi.fn(),
    loadOlderThreads: vi.fn(),
    reloadAfterFilterChange: vi.fn(),
    filterChangedSinceLoad: () => false,
    ensureThreadInMap: vi.fn(),
    refreshStaleThreadEvents: vi.fn(),
}));
vi.mock('../../../store/actions/archive-all', () => ({ handleArchiveAll: vi.fn() }));

let host: HTMLDivElement;
const tiles = () => Array.from(host.querySelectorAll<HTMLElement>('.drawer-ongoing-tiles .ongoing-tile'));
const labels = () => tiles().map(t => t.querySelector('.ongoing-tile-label')!.textContent);
const tile = (label: string) => tiles().find(t => t.querySelector('.ongoing-tile-label')!.textContent === label)!;
const count = (label: string) => tile(label).querySelector('.ongoing-tile-count')!.textContent;
const selectedLabels = () => tiles().filter(t => t.getAttribute('aria-current') === 'true').map(t => t.querySelector('.ongoing-tile-label')!.textContent);
const rowIds = () => Array.from(host.querySelectorAll('.ongoing-grouping [data-thread-nav]')).map(r => r.getAttribute('data-thread-nav'));
const emptyState = () => host.querySelector('.ongoing-grouping .empty-state')?.textContent;
const ALL = ['Blocked', 'Review', 'Drafts', 'In flight'];

function mount(): void {
    act(() => { render(<ThreadDrawer forceVisible />, host); });
}

beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    _resetComposeDraftsForTesting();
    setThreadChannelFilter(new Set(ALL_CHANNELS));
    threadSearchQuery.value = '';
    threadSearchResults.value = { status: 'not-loaded' };
    setDrawerGrouping('ongoing');
    setSelectedOngoingGroup('blocked');
    focusedThreadId.value = null;
    threadMap.value = new Map([
        ['q', makeThreadState('q', { meta: { section: 'inbox', status: 'waiting_for_user_answer', title: 'Question', createdAt: '2026-01-04T00:00:00Z' } })],
        ['run', makeThreadState('run', { meta: { section: 'inbox', status: 'running', channel: 'trigger', title: 'Running trigger', createdAt: '2026-01-03T00:00:00Z' } })],
        ['quiet', makeThreadState('quiet', { meta: { section: 'inbox', title: 'Quiet one', createdAt: '2026-01-02T00:00:00Z' } })],
    ]);
    threadsLoaded.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
});

afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    setDrawerGrouping('folders');
    setSelectedOngoingGroup('blocked');
    vi.unstubAllGlobals();
});

describe('the Ongoing grouping', () => {
    it('draws all four tiles in the fixed order, each with a count, an icon and a name', () => {
        mount();
        expect(labels()).toEqual(ALL);
        expect(ALL.map(count)).toEqual(['1', '0', '0', '1']);
        for (const t of tiles()) expect(t.querySelector('.ongoing-tile-icon svg')).not.toBeNull();
        expect(tile('Drafts').classList.contains('ongoing-tile-empty')).toBe(true);
        expect(tile('In flight').classList.contains('ongoing-tile-empty')).toBe(false);
    });

    it('lists only the selected group, below the tiles', () => {
        mount();
        expect(selectedLabels()).toEqual(['Blocked']);
        expect(rowIds()).toEqual(['q']);
        const row = host.querySelector('.ongoing-grouping [data-thread-nav="q"]')!;
        const grid = host.querySelector('.drawer-ongoing-tiles')!;
        expect(grid.compareDocumentPosition(row) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    });

    it('selects a group on a press, and a press on the selected one keeps it', () => {
        mount();
        act(() => { tile('In flight').click(); });
        expect(selectedOngoingGroup.value).toBe('in-flight');
        expect(selectedLabels()).toEqual(['In flight']);
        expect(rowIds()).toEqual(['run']);
        act(() => { tile('Blocked').click(); });
        act(() => { tile('Blocked').click(); });
        expect(selectedOngoingGroup.value).toBe('blocked');
        expect(rowIds()).toEqual(['q']);
    });

    describe('a press on the selected In flight tile', () => {
        beforeEach(() => {
            threadMap.value = new Map([
                ...threadMap.value,
                ['parked', makeThreadState('parked', { meta: { section: 'inbox', liveEventWaitCount: 1, title: 'Parked', createdAt: '2026-01-05T00:00:00Z' } })],
            ]);
        });
        const listed = () => Array.from(tile('In flight').querySelectorAll('.in-flight-breakdown-part-listed')).map(p => p.textContent);

        it('lists only the running rows, then only the waiting ones, then all', () => {
            mount();
            act(() => { tile('In flight').click(); });
            expect(rowIds().sort()).toEqual(['parked', 'run']);
            expect(listed()).toEqual([]);
            act(() => { tile('In flight').click(); });
            expect(rowIds()).toEqual(['run']);
            expect(listed()).toEqual(['1 running']);
            expect(tile('In flight').getAttribute('aria-label')).toBe('In flight, 2, showing running');
            act(() => { tile('In flight').click(); });
            expect(rowIds()).toEqual(['parked']);
            expect(listed()).toEqual(['1 waiting']);
            act(() => { tile('In flight').click(); });
            expect(rowIds().sort()).toEqual(['parked', 'run']);
            expect(listed()).toEqual([]);
        });

        it('keeps the tile count at the whole group', () => {
            setSelectedOngoingGroup('in-flight');
            mount();
            act(() => { tile('In flight').click(); });
            expect(count('In flight')).toBe('2');
        });

        it('opens the first row the filter keeps', () => {
            setSelectedOngoingGroup('in-flight');
            mount();
            act(() => { tile('In flight').click(); });
            expect(focusedThreadId.value).toBe('run');
            act(() => { tile('In flight').click(); });
            expect(focusedThreadId.value).toBe('parked');
        });

        it('says which kind it is missing when the filter keeps nothing', () => {
            threadMap.value = new Map([...threadMap.value].filter(([id]) => id !== 'run'));
            setSelectedOngoingGroup('in-flight');
            mount();
            act(() => { tile('In flight').click(); });
            expect(rowIds()).toEqual([]);
            expect(emptyState()).toBe('Nothing running');
            act(() => { tile('In flight').click(); });
            expect(rowIds()).toEqual(['parked']);
        });

        it('lists the whole group again once another group is selected', () => {
            setSelectedOngoingGroup('in-flight');
            mount();
            act(() => { tile('In flight').click(); });
            expect(rowIds()).toEqual(['run']);
            act(() => { tile('Blocked').click(); });
            act(() => { tile('In flight').click(); });
            expect(rowIds().sort()).toEqual(['parked', 'run']);
        });
    });

    it('opens the group\'s first thread on a press', () => {
        mount();
        act(() => { tile('In flight').click(); });
        expect(focusedThreadId.value).toBe('run');
        act(() => { tile('Blocked').click(); });
        expect(focusedThreadId.value).toBe('q');
    });

    it('opens a draft from the Drafts tile', () => {
        threadMap.value = new Map([
            ...threadMap.value,
            ['draft', makeThreadState('draft', { meta: { section: 'inbox', state: 'composing', title: 'Draft', createdAt: '2026-01-05T00:00:00Z' } })],
        ]);
        patchDraft('draft', { text: 'half a thought' });
        mount();
        act(() => { tile('Drafts').click(); });
        expect(focusedThreadId.value).toBe('draft');
    });

    it('opens the only thread of a group on the phone', () => {
        vi.stubGlobal('innerWidth', 375);
        vi.stubGlobal('innerHeight', 812);
        mount();
        act(() => { tile('In flight').click(); });
        expect(selectedOngoingGroup.value).toBe('in-flight');
        expect(focusedThreadId.value).toBe('run');
    });

    it('only selects on the phone before the threads load, when a sole row may not be the whole group', () => {
        threadsLoaded.value = false;
        vi.stubGlobal('innerWidth', 375);
        vi.stubGlobal('innerHeight', 812);
        mount();
        act(() => { tile('In flight').click(); });
        expect(selectedOngoingGroup.value).toBe('in-flight');
        expect(focusedThreadId.value).toBeNull();
    });

    it('only selects a group of several on the phone, where opening would swipe off the list', () => {
        threadMap.value = new Map([
            ...threadMap.value,
            ['run2', makeThreadState('run2', { meta: { section: 'inbox', status: 'running', title: 'Second run', createdAt: '2026-01-05T00:00:00Z' } })],
        ]);
        vi.stubGlobal('innerWidth', 375);
        vi.stubGlobal('innerHeight', 812);
        mount();
        act(() => { tile('In flight').click(); });
        expect(selectedOngoingGroup.value).toBe('in-flight');
        expect(rowIds()).toHaveLength(2);
        expect(focusedThreadId.value).toBeNull();
    });

    it('only selects an empty group, leaving the open thread as it was', () => {
        focusedThreadId.value = 'quiet';
        mount();
        act(() => { tile('Review').click(); });
        expect(selectedOngoingGroup.value).toBe('review');
        expect(focusedThreadId.value).toBe('quiet');
    });

    it('keeps an empty group selected and says it is empty', () => {
        setSelectedOngoingGroup('review');
        mount();
        expect(selectedLabels()).toEqual(['Review']);
        expect(rowIds()).toEqual([]);
        expect(emptyState()).toBe('Nothing to review');
    });

    it('claims nothing before the threads load', () => {
        threadsLoaded.value = false;
        mount();
        expect(labels()).toEqual(ALL);
        expect(ALL.map(count)).toEqual(['', '', '', '']);
        expect(emptyState()).toBeUndefined();
    });

    it('has no Idle group, and a thread matching no status shows in none', () => {
        setSelectedOngoingGroup('in-flight');
        mount();
        expect(labels()).not.toContain('Idle');
        expect(host.querySelector('.ongoing-grouping .drawer-archive-all')).toBeNull();
        expect(rowIds()).not.toContain('quiet');
    });

    it('walks the tiles, then the rows, with the keyboard', () => {
        mount();
        const highlighted = () => host.querySelector('.ongoing-tile-highlighted .ongoing-tile-label')?.textContent;
        act(() => { moveHighlight(1); });
        expect(highlighted()).toBe('Blocked');
        // → and ← step along the tiles without selecting.
        act(() => { expandHighlighted(); });
        expect(highlighted()).toBe('Review');
        act(() => { collapseHighlighted(); });
        expect(highlighted()).toBe('Blocked');
        expect(selectedOngoingGroup.value).toBe('blocked');
        // Enter presses the highlighted tile: it selects and opens the first row.
        act(() => { moveHighlight(1); moveHighlight(1); moveHighlight(1); });
        expect(highlighted()).toBe('In flight');
        act(() => { selectHighlighted(); });
        expect(selectedOngoingGroup.value).toBe('in-flight');
        expect(focusedThreadId.value).toBe('run');
        // ↓ from the last tile reaches the selected group's row.
        act(() => { moveHighlight(1); });
        expect(host.querySelector('.thread-row-highlighted')?.getAttribute('data-thread-nav')).toBe('run');
        // ← from the row returns to its tile.
        act(() => { collapseHighlighted(); });
        expect(highlighted()).toBe('In flight');
    });

    it('lists a thread the thread-type filter hides under Folders', () => {
        setThreadChannelFilter(new Set(['chat']));
        setSelectedOngoingGroup('in-flight');
        mount();
        expect(rowIds()).toEqual(['run']);
    });

    it('names the In flight split in words on its tile, and shimmers only while a row runs', () => {
        mount();
        const inFlight = tile('In flight');
        expect(inFlight.querySelector('.in-flight-breakdown')?.textContent).toBe('1 running');
        expect(inFlight.querySelector('.ongoing-tile-label')!.classList.contains('running-shimmer')).toBe(true);
        expect(tile('Blocked').querySelector('.ongoing-tile-label')!.classList.contains('running-shimmer')).toBe(false);
    });

    it('labels each tile with its name and count for assistive tech', () => {
        mount();
        expect(tile('Blocked').getAttribute('role')).toBe('treeitem');
        expect(tile('Blocked').getAttribute('aria-label')).toBe('Blocked, 1');
        expect(tile('Drafts').getAttribute('aria-current')).toBeNull();
    });

    it('keeps Archive all on the Current header under Folders', () => {
        setDrawerGrouping('folders');
        mount();
        const current = Array.from(host.querySelectorAll<HTMLElement>('.list-section-title-collapsible'))
            .find(h => h.querySelector('.section-label')?.textContent === 'Current')!;
        act(() => { current.querySelector<HTMLButtonElement>('.drawer-archive-all')!.click(); });
        expect(handleArchiveAll).toHaveBeenCalledTimes(1);
        expect(current.getAttribute('aria-expanded')).toBe('true');
    });

    it('gives way to search, which shows its own results', () => {
        threadSearchQuery.value = 'question';
        threadSearchResults.value = { status: 'loaded', data: [] };
        mount();
        expect(host.querySelector('.ongoing-grouping')).toBeNull();
        expect(host.querySelector('.thread-drawer-list')!.textContent).toContain('No threads found');
    });
});

describe("the grouping button's Blocked count", () => {
    const badge = () => host.querySelector('.grouping-btn .badge');

    it('rides the button under Folders, where it offers Ongoing', () => {
        setDrawerGrouping('folders');
        act(() => { render(<ThreadGroupingButton />, host); });
        expect(badge()?.textContent).toBe('1');
        expect(host.querySelector('.grouping-btn')!.getAttribute('aria-label')).toBe('Show Ongoing (1 blocked)');
        act(() => { setDrawerGrouping('ongoing'); });
        expect(badge()).toBeNull();
    });

    it('is absent at 0', () => {
        setDrawerGrouping('folders');
        threadMap.value = new Map([['quiet', makeThreadState('quiet', { meta: { section: 'inbox' } })]]);
        act(() => { render(<ThreadGroupingButton />, host); });
        expect(badge()).toBeNull();
    });
});
