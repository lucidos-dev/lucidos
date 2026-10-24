// @vitest-environment jsdom
/**
 * Recently applied pages in older changes as its last row scrolls into view.
 * The sentinel that asks lives inside the collapsible section, so opening the
 * section must observe the sentinel it mounts.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
  focusThread: vi.fn(),
}));
vi.mock('../../../store/actions/repositories', () => ({
  viewChangeDiff: vi.fn(),
  viewThreadCcDiff: vi.fn(),
}));

import { ChangesView } from '../ChangesView';
import {
  changes,
  appliedChanges,
  setAsideChanges,
  changesHasMore,
  collapsedChangesSectionIds,
  toggleChangesSectionCollapsed,
} from '../../../store/store';
import type { Change } from '../../../api/client';

function applied(id: string): Change {
  return {
    id,
    request_id: '00000000-0000-0000-0000-000000000000',
    thread_id: `thread-${id}`,
    thread_title: 'Thread',
    branch_name: 'b',
    repo_root: '/r',
    description: 'desc',
    file_count: 1,
    files: ['a.rs'],
    requires_restart: false,
    hardened: true,
    needs_hardening: false,
    apply_ready: true,
    status: 'applied',
    created_at: '2026-01-01T00:00:00Z',
    resolved_at: '2026-01-01T01:00:00Z',
    pre_merge_sha: null,
    post_merge_sha: null,
    commits: [],
    summary: null,
    incomplete: false,
  };
}

const observed: Element[] = [];

class RecordingObserver {
  observe(el: Element) { observed.push(el); }
  disconnect() {}
  unobserve() {}
  takeRecords() { return []; }
}

let host: HTMLDivElement;

beforeEach(() => {
  observed.length = 0;
  vi.stubGlobal('IntersectionObserver', RecordingObserver);
  localStorage.clear();
  changes.value = { status: 'loaded', data: [] };
  setAsideChanges.value = { status: 'loaded', data: [] };
  appliedChanges.value = { status: 'loaded', data: [applied('a'), applied('b')] };
  changesHasMore.value = true;
  collapsedChangesSectionIds.value = new Set(['applied']);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  changesHasMore.value = false;
});

describe('Recently applied loads older changes', () => {
  it('observes the sentinel a collapsed section mounts when it opens', async () => {
    await act(() => { render(<ChangesView />, host); });
    expect(host.querySelector('.dropdown-panel-loading-more')).toBeNull();
    expect(observed).toEqual([]);

    await act(() => { toggleChangesSectionCollapsed('applied'); });
    const sentinel = host.querySelector('.dropdown-panel-loading-more');
    expect(sentinel).not.toBeNull();
    expect(observed).toContain(sentinel);
  });
});
