import { describe, it, expect, beforeEach } from 'vitest';
import {
  appsList,
  threadMap,
  selectedAppIds,
  filterFacets,
  includeDeletedFilterOptions,
} from './store';
import { appFilterOptions, appIdsMissingFromFacets, widgetFilterOptions } from './appFilters';
import type { AppFilterFacet } from '../api/threads';
import type { App } from './types';
import type { ThreadState } from './thread-events';

const habitTracker: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false };

function appThread(folder: string | undefined): ThreadState {
  return {
    meta: { codingAgentKind: 'app', codingAgentFolder: folder, updatedAt: '2026-05-01T00:00:00Z' },
  } as unknown as ThreadState;
}

function facet(id: string, fields: Partial<AppFilterFacet>): AppFilterFacet {
  return { id, name: null, kind: null, deleted: false, last_activity: '2026-04-01T00:00:00Z', ...fields };
}

function facets(...apps: AppFilterFacet[]): void {
  filterFacets.value = { status: 'loaded', data: { triggers: [], repos: [], apps } };
}

beforeEach(() => {
  appsList.value = { status: 'loaded', data: [habitTracker] };
  threadMap.value = new Map();
  selectedAppIds.value = new Set();
  filterFacets.value = { status: 'not-loaded' };
  // These cases assert deleted-entry labelling, which is independent of the
  // include-deleted toggle, so deleted rows are listed.
  includeDeletedFilterOptions.value = true;
});

describe('appFilterOptions: the engine labels each facet', () => {
  it('labels a live app by the name the facet carries', () => {
    facets(facet('habit-tracker', { name: 'Habit Tracker', kind: 'app' }));
    expect(appFilterOptions.value).toEqual([
      { id: 'habit-tracker', label: 'Habit Tracker', deleted: false, widget: false, lastActivity: undefined },
    ]);
    expect(widgetFilterOptions.value).toEqual([]);
  });

  it('lists a live widget under Widgets by its manifest name, not as deleted', () => {
    facets(facet('fare-grid', { name: 'Fare grid', kind: 'widget' }));
    expect(widgetFilterOptions.value).toEqual([
      { id: 'fare-grid', label: 'Fare grid', deleted: false, widget: true, lastActivity: undefined },
    ]);
    expect(appFilterOptions.value).toEqual([]);
  });

  it('keeps a deleted widget under Widgets, marked deleted', () => {
    facets(facet('fare-grid', { kind: 'widget', deleted: true }));
    expect(widgetFilterOptions.value).toEqual([
      { id: 'fare-grid', label: 'fare-grid', deleted: true, widget: true, lastActivity: '2026-04-01T00:00:00Z' },
    ]);
  });

  it('lists a deleted folder of unknown kind under Apps, marked deleted', () => {
    facets(facet('gone', { deleted: true }));
    expect(appFilterOptions.value).toMatchObject([{ id: 'gone', label: 'gone', deleted: true, widget: false }]);
  });

  it('labels a thread the facets do not cover yet from appsList', () => {
    facets();
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/habit-tracker')]]);
    expect(appFilterOptions.value).toEqual([
      { id: 'habit-tracker', label: 'Habit Tracker', deleted: false, widget: false },
    ]);
  });

  it('leaves out an id nothing labels yet, rather than call it deleted', () => {
    facets();
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/fare-grid')]]);
    expect(appFilterOptions.value).toEqual([]);
    expect(widgetFilterOptions.value).toEqual([]);
  });

  it('keeps an unlabelled selected id under its id, so it stays clearable', () => {
    selectedAppIds.value = new Set(['fare-grid']);
    expect(appFilterOptions.value).toEqual([
      { id: 'fare-grid', label: 'fare-grid', deleted: false, widget: false },
    ]);
  });

  it('ignores non-app coding-agent threads', () => {
    threadMap.value = new Map([
      ['t1', { meta: { codingAgentKind: 'lucidos', codingAgentFolder: '/ws/data/apps/habit-tracker' } } as unknown as ThreadState],
    ]);
    expect(appFilterOptions.value).toEqual([]);
  });
});

describe('appIdsMissingFromFacets', () => {
  it('is empty until the facets load', () => {
    threadMap.value = new Map([['t1', appThread('/ws/data/apps/fare-grid')]]);
    expect(appIdsMissingFromFacets.value).toEqual([]);
  });

  it('names a loaded thread\'s app the facets do not label', () => {
    facets(facet('habit-tracker', { name: 'Habit Tracker', kind: 'app' }));
    threadMap.value = new Map([
      ['t1', appThread('/ws/data/apps/habit-tracker')],
      ['t2', appThread('/ws/data/apps/fare-grid')],
    ]);
    expect(appIdsMissingFromFacets.value).toEqual(['fare-grid']);
  });
});

describe('appFilterOptions: include-deleted toggle', () => {
  beforeEach(() => {
    facets(
      facet('habit-tracker', { name: 'Habit Tracker', kind: 'app' }),
      facet('gone', { deleted: true }),
    );
  });

  it('excludes deleted apps when the toggle is off (default)', () => {
    includeDeletedFilterOptions.value = false;
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker']);
  });

  it('includes deleted apps when the toggle is on', () => {
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker', 'gone']);
  });

  it('keeps a selected deleted app visible even when the toggle is off (stays clearable)', () => {
    includeDeletedFilterOptions.value = false;
    selectedAppIds.value = new Set(['gone']);
    expect(appFilterOptions.value.map(o => o.id)).toEqual(['habit-tracker', 'gone']);
  });
});
