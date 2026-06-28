import { describe, it, expect, beforeEach } from 'vitest';
import {
  appsList,
  threadMap,
  selectedAppIds,
  filterFacets,
  includeDeletedFilterOptions,
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
    // These cases assert deleted-entry labeling/sorting, which is independent
    // of the include-deleted toggle — opt in so deleted rows are listed.
    includeDeletedFilterOptions.value = true;
  });

  it('returns [] until appsList loads', () => {
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/habit-tracker')]]);
    expect(appFilterOptions.value).toEqual([]);
  });

  it('omits an app that has no CC session', () => {
    appsList.value = {
      status: 'loaded',
      data: [
        { id: 'habit-tracker', name: 'Habit Tracker', description: '' },
        { id: 'habit', name: 'Habit', description: '' },
      ],
    };
    // Only habit-tracker has a thread.
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/habit-tracker')]]);
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker']);
  });

  it('labels a session app from the live appsList (not deleted)', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '' }],
    };
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/habit-tracker')]]);
    expect(appFilterOptions.value).toEqual([{ id: 'habit-tracker', label: 'Habit Tracker', deleted: false }]);
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
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '' }],
    };
    threadMap.value = new Map();
    selectedAppIds.value = new Set(['habit-tracker']);
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker']);
  });

  it('ignores non-app coding-agent threads', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '' }],
    };
    threadMap.value = new Map([
      ['t1', { meta: { codingAgentKind: 'lucidos', codingAgentFolder: '/ws/data/apps/habit-tracker' } } as unknown as ThreadState],
    ]);
    expect(appFilterOptions.value).toEqual([]);
  });

  it('lists an app from filterFacets even with no loaded thread (complete option list)', () => {
    appsList.value = {
      status: 'loaded',
      data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '' }],
    };
    threadMap.value = new Map();
    filterFacets.value = {
      status: 'loaded',
      data: { triggers: [], repos: [], apps: [{ id: 'habit-tracker', name: null, last_activity: '2026-04-01T00:00:00Z' }] },
    };
    expect(appFilterOptions.value).toEqual([{ id: 'habit-tracker', label: 'Habit Tracker', deleted: false }]);
  });

  it('marks a facet app missing from appsList as deleted', () => {
    appsList.value = { status: 'loaded', data: [] };
    filterFacets.value = {
      status: 'loaded',
      data: { triggers: [], repos: [], apps: [{ id: 'gone', name: null, last_activity: '2026-04-01T00:00:00Z' }] },
    };
    const opt = appFilterOptions.value;
    expect(opt).toHaveLength(1);
    expect(opt[0]).toMatchObject({ id: 'gone', label: 'gone', deleted: true });
  });
});

describe('appFilterOptions — include-deleted toggle', () => {
  beforeEach(() => {
    appsList.value = { status: 'loaded', data: [{ id: 'habit-tracker', name: 'Habit Tracker', description: '' }] };
    threadMap.value = new Map([
      ['t1', appThread('/ws/data/apps/habit-tracker')],
      ['t2', appThread('/ws/data/apps/gone')],
    ]);
    selectedAppIds.value = new Set();
    filterFacets.value = { status: 'not-loaded' };
  });

  it('excludes deleted apps when the toggle is off (default)', () => {
    includeDeletedFilterOptions.value = false;
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker']);
  });

  it('includes deleted apps when the toggle is on', () => {
    includeDeletedFilterOptions.value = true;
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker', 'gone']);
  });

  it('keeps a selected deleted app visible even when the toggle is off (stays clearable)', () => {
    includeDeletedFilterOptions.value = false;
    selectedAppIds.value = new Set(['gone']);
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker', 'gone']);
  });
});
