import { describe, it, expect, beforeEach } from 'vitest';
import {
  repositories,
  threadMap,
  selectedRepoIds,
  filterFacets,
} from './store';
import { repoFilterOptions } from './repoFilters';
import type { ThreadState } from './thread-events';

function ccThread(repoId: string | undefined, repoName?: string): ThreadState {
  return {
    meta: { channel: 'claude_code', repoId, repoName, updatedAt: '2026-05-01T00:00:00Z' },
  } as unknown as ThreadState;
}

describe('repoFilterOptions — only repos with CC sessions', () => {
  beforeEach(() => {
    repositories.value = { status: 'not-loaded' };
    threadMap.value = new Map();
    selectedRepoIds.value = new Set();
    filterFacets.value = { status: 'not-loaded' };
  });

  it('returns [] until the registry loads', () => {
    threadMap.value = new Map([['t1', ccThread('r1')]]);
    expect(repoFilterOptions.value).toEqual([]);
  });

  it('omits a registered repo that has no CC session', () => {
    repositories.value = {
      status: 'loaded',
      data: [
        { id: 'r1', name: 'Repo 1', path: '/r1' },
        { id: 'r2', name: 'Repo 2', path: '/r2' },
      ],
    };
    // Only r1 has a thread.
    threadMap.value = new Map([['t1', ccThread('r1')]]);
    const ids = repoFilterOptions.value.map(o => o.id);
    expect(ids).toEqual(['r1']);
  });

  it('labels a session repo from the live registry (not deleted)', () => {
    repositories.value = { status: 'loaded', data: [{ id: 'r1', name: 'Repo 1', path: '/r1' }] };
    threadMap.value = new Map([['t1', ccThread('r1')]]);
    expect(repoFilterOptions.value).toEqual([{ id: 'r1', label: 'Repo 1', deleted: false }]);
  });

  it('marks a session repo missing from the registry as deleted', () => {
    repositories.value = { status: 'loaded', data: [] };
    threadMap.value = new Map([['t1', ccThread('gone', 'Gone Repo')]]);
    const opt = repoFilterOptions.value;
    expect(opt).toHaveLength(1);
    expect(opt[0]).toMatchObject({ id: 'gone', label: 'Gone Repo', deleted: true });
  });

  it('keeps a selected repo even with no session so it stays clearable', () => {
    repositories.value = { status: 'loaded', data: [{ id: 'r1', name: 'Repo 1', path: '/r1' }] };
    threadMap.value = new Map();
    selectedRepoIds.value = new Set(['r1']);
    expect(repoFilterOptions.value.map(o => o.id)).toEqual(['r1']);
  });

  it('ignores non-claude_code threads', () => {
    repositories.value = { status: 'loaded', data: [{ id: 'r1', name: 'Repo 1', path: '/r1' }] };
    threadMap.value = new Map([
      ['t1', { meta: { channel: 'chat', repoId: 'r1' } } as unknown as ThreadState],
    ]);
    expect(repoFilterOptions.value).toEqual([]);
  });
});
