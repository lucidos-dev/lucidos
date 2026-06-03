import { describe, it, expect, beforeEach } from 'vitest';
import {
  appsList,
  threadMap,
  selectedAppIds,
  filterFacets,
} from './store';
import { appFilterOptions } from './appFilters';
import type { ThreadState } from './thread-events';

function appThread(folder: string | undefined): ThreadState {
  return {
    meta: { codingAgentKind: 'app', codingAgentFolder: folder, updatedAt: '2026-05-01T00:00:00Z' },
  } as unknown as ThreadState;
}

describe('appFilterOptions — only apps with CC sessions', () => {
  beforeEach(() => {
    appsList.value = { status: 'not-loaded' };
    threadMap.value = new Map();
    selectedAppIds.value = new Set();
    filterFacets.value = { status: 'not-loaded' };
  });

  it('returns [] until appsList loads', () => {
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/momentum')]]);
    expect(appFilterOptions.value).toEqual([]);
  });

  it('omits an app that has no CC session', () => {
    appsList.value = {
      status: 'loaded',
      data: [
        { id: 'momentum', name: 'Momentum', description: '', knowhow: [] },
        { id: 'habit', name: 'Habit', description: '', knowhow: [] },
      ],
    };
    // Only momentum has a thread.
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/momentum')]]);
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['momentum']);
  });

  it('labels a session app from the live appsList (not deleted)', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'momentum', name: 'Momentum', description: '', knowhow: [] }],
    };
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/momentum')]]);
    expect(appFilterOptions.value).toEqual([{ id: 'momentum', label: 'Momentum', deleted: false }]);
  });

  it('marks a session app missing from appsList as deleted', () => {
    appsList.value = { status: 'loaded', data: [] };
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/gone')]]);
    const opt = appFilterOptions.value;
    expect(opt).toHaveLength(1);
    expect(opt[0]).toMatchObject({ id: 'gone', label: 'gone', deleted: true });
  });

  it('keeps a selected app even with no session so it stays clearable', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'momentum', name: 'Momentum', description: '', knowhow: [] }],
    };
    threadMap.value = new Map();
    selectedAppIds.value = new Set(['momentum']);
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['momentum']);
  });

  it('ignores non-app coding-agent threads', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'momentum', name: 'Momentum', description: '', knowhow: [] }],
    };
    threadMap.value = new Map([
      ['t1', { meta: { codingAgentKind: 'lucidos', codingAgentFolder: '/ws/data/apps/momentum' } } as unknown as ThreadState],
    ]);
    expect(appFilterOptions.value).toEqual([]);
  });

  it('lists an app from filterFacets even with no loaded thread (complete option list)', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'momentum', name: 'Momentum', description: '', knowhow: [] }],
    };
    threadMap.value = new Map();
    filterFacets.value = {
      status: 'loaded',
      data: { triggers: [], repos: [], apps: [{ id: 'momentum', last_activity: '2026-04-01T00:00:00Z' }] },
    };
    expect(appFilterOptions.value).toEqual([{ id: 'momentum', label: 'Momentum', deleted: false }]);
  });

  it('marks a facet app missing from appsList as deleted', () => {
    appsList.value = { status: 'loaded', data: [] };
    filterFacets.value = {
      status: 'loaded',
      data: { triggers: [], repos: [], apps: [{ id: 'gone', last_activity: '2026-04-01T00:00:00Z' }] },
    };
    const opt = appFilterOptions.value;
    expect(opt).toHaveLength(1);
    expect(opt[0]).toMatchObject({ id: 'gone', label: 'gone', deleted: true });
  });
});
