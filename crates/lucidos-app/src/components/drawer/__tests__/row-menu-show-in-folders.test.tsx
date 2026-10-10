// @vitest-environment jsdom
/**
 * A drawer row's ⋯ menu offers Show in Folders under the Ongoing grouping.
 * Under Folders the row already is the thread's place, so the item would
 * point at itself.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer } from '../ThreadDrawer';
import {
    drawerGrouping, setDrawerGrouping, setSelectedOngoingGroup, threadMap, threadsLoaded, focusedThreadId,
    threadSearchQuery, threadSearchResults, setThreadChannelFilter, ALL_CHANNELS,
} from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { _resetComposeDraftsForTesting } from '../../../store/composeDrafts';
import type { ThreadSearchResult } from '../../../api/threads';

vi.mock('../../../store/actions/thread-loading', () => ({
    loadThreadEvents: vi.fn(),
    loadOlderThreads: vi.fn(),
    reloadAfterFilterChange: vi.fn(),
    filterChangedSinceLoad: () => false,
    ensureThreadInMap: vi.fn(),
    refreshStaleThreadEvents: vi.fn(),
}));

let host: HTMLDivElement;

function mount(): void {
    act(() => { render(<ThreadDrawer forceVisible />, host); });
}

function openRowMenu(id: string): HTMLElement[] {
    const row = host.querySelector<HTMLElement>(`[data-thread-nav="${id}"]`);
    const trigger = row?.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
    expect(trigger).toBeTruthy();
    act(() => { trigger!.click(); });
    return [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
}

const labels = (items: HTMLElement[]) => items.map(el => el.textContent);

/** The fields a search row reads; the rest of the summary stays unset. */
function searchHit(id: string): ThreadSearchResult {
    return {
        thread_id: id, title: 'Question', channel: 'chat', status: 'idle', section: 'inbox',
        created_at: '2026-01-01T00:00:00Z', score: 1,
    } as ThreadSearchResult;
}

beforeEach(() => {
    vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    localStorage.clear();
    _resetComposeDraftsForTesting();
    setThreadChannelFilter(new Set(ALL_CHANNELS));
    threadSearchQuery.value = '';
    threadSearchResults.value = { status: 'not-loaded' };
    setSelectedOngoingGroup('blocked');
    focusedThreadId.value = null;
    threadMap.value = new Map([
        ['q', makeThreadState('q', { meta: { section: 'inbox', status: 'waiting_for_user_answer', title: 'Question' } })],
    ]);
    threadsLoaded.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
});

afterEach(() => {
    act(() => { render(null, host); });
    host.remove();
    document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
    setDrawerGrouping('folders');
    vi.unstubAllGlobals();
});

describe('Show in Folders on a drawer row', () => {
    it('leads the menu under Ongoing and lands the thread in Folders', () => {
        setDrawerGrouping('ongoing');
        mount();
        const items = openRowMenu('q');
        expect(items[0]?.textContent).toBe('Show in Folders');

        act(() => { items[0].click(); });

        expect(drawerGrouping.value).toBe('folders');
    });

    it('is absent on a search hit, even with Ongoing as the grouping behind the search', () => {
        setDrawerGrouping('ongoing');
        threadSearchQuery.value = 'Question';
        threadSearchResults.value = { status: 'loaded', data: [searchHit('q')] };
        mount();
        expect(labels(openRowMenu('q'))).not.toContain('Show in Folders');
    });

    it('is absent under Folders, where the row is the thread place', () => {
        setDrawerGrouping('folders');
        mount();
        const items = openRowMenu('q');
        expect(labels(items)).not.toContain('Show in Folders');
        expect(items.length).toBeGreaterThan(0);
    });
});
