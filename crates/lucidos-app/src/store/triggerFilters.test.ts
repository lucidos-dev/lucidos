import { describe, it, expect, beforeEach } from 'vitest';
import { triggers, historicalTriggers, threadMap, selectedTriggerIds, includeDeletedFilterOptions } from './store';
import { triggerFilterOptions } from './triggerFilters';
import { makeOptimisticThreadState } from './thread-events';
import { makeTrigger } from './__tests__/fixtures';
import type { ThreadState } from './thread-events';

function makeTriggerThread(id: string, triggerId: string, triggerName?: string): ThreadState {
  return makeOptimisticThreadState({
    id,
    title: 'Trigger run',
    channel: 'trigger',
    initiator: 'system',
    eventsLoaded: false,
    triggerId,
    triggerName,
  });
}

describe('triggerFilterOptions', () => {
  beforeEach(() => {
    triggers.value = { status: 'not-loaded' };
    historicalTriggers.value = { status: 'not-loaded' };
    threadMap.value = new Map();
    selectedTriggerIds.value = new Set();
    // These cases assert deleted-entry labeling/sorting, which is independent
    // of the include-deleted toggle — opt in so deleted rows are listed.
    includeDeletedFilterOptions.value = true;
  });

  it('returns empty list while the registry is still loading', () => {
    triggers.value = { status: 'loading' };
    const map = new Map<string, ThreadState>();
    map.set('t1', makeTriggerThread('t1', 'live-1', 'Live One'));
    threadMap.value = map;

    // Without the registry we cannot tell live from deleted — refuse to render
    // children rather than mis-label every live trigger as "(deleted)".
    expect(triggerFilterOptions.value).toEqual([]);
  });

  it('returns empty list when the registry has not loaded yet', () => {
    triggers.value = { status: 'not-loaded' };
    const map = new Map<string, ThreadState>();
    map.set('t1', makeTriggerThread('t1', 'live-1', 'Live One'));
    threadMap.value = map;

    expect(triggerFilterOptions.value).toEqual([]);
  });

  it('shows all live intent triggers, even those without threads', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'live-1', name: 'Live One' }),
        makeTrigger({ id: 'live-2', name: 'Live Two (no threads yet)' }),
      ],
    };
    const map = new Map<string, ThreadState>();
    map.set('t1', makeTriggerThread('t1', 'live-1', 'Live One'));
    threadMap.value = map;

    expect(triggerFilterOptions.value).toEqual([
      { id: 'live-1', label: 'Live One', deleted: false },
      { id: 'live-2', label: 'Live Two (no threads yet)', deleted: false },
    ]);
  });

  it('hides live script triggers that have no threads', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'live-1', name: 'Live Intent' }),
        makeTrigger({
          id: 'script-1',
          name: 'Nightly Cleanup Script',
          run: { type: 'script', path: 'cleanup.sh' },
        }),
      ],
    };
    threadMap.value = new Map();

    expect(triggerFilterOptions.value).toEqual([
      { id: 'live-1', label: 'Live Intent', deleted: false },
    ]);
  });

  it('marks trigger as deleted when its id is missing from the loaded registry', () => {
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'live-1', name: 'Live One' })] };
    const map = new Map<string, ThreadState>();
    map.set('t1', makeTriggerThread('t1', 'live-1', 'Live One'));
    map.set('t2', makeTriggerThread('t2', 'gone-1', 'Old Trigger'));
    threadMap.value = map;

    expect(triggerFilterOptions.value).toEqual([
      { id: 'live-1', label: 'Live One', deleted: false },
      { id: 'gone-1', label: 'Old Trigger', deleted: true, lastActivity: undefined },
    ]);
  });

  it('attaches lastActivity to deleted entries from the historical-triggers projection', () => {
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'live-1', name: 'Live One' })] };
    historicalTriggers.value = {
      status: 'loaded',
      data: [
        { id: 'live-1', name: 'Live One', last_activity: '2026-04-30T04:06:05Z' },
        { id: 'gone-historical', name: 'Long-Gone Trigger', last_activity: '2026-04-15T22:30:00Z' },
      ],
    };
    threadMap.value = new Map();

    expect(triggerFilterOptions.value).toEqual([
      { id: 'live-1', label: 'Live One', deleted: false },
      {
        id: 'gone-historical',
        label: 'Long-Gone Trigger',
        deleted: true,
        lastActivity: '2026-04-15T22:30:00Z',
      },
    ]);
  });

  it('always includes selected ids even if no loaded thread references them', () => {
    // Restored from localStorage on a fresh load — without this the filter
    // applies silently and the user sees an empty list with no checkbox to clear.
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'live-1', name: 'Live One' })] };
    threadMap.value = new Map();
    selectedTriggerIds.value = new Set(['live-1', 'gone-1']);

    expect(triggerFilterOptions.value).toEqual([
      { id: 'live-1', label: 'Live One', deleted: false },
      { id: 'gone-1', label: 'gone-1', deleted: true, lastActivity: undefined },
    ]);
  });

  it('sorts live triggers before deleted, deleted by recency desc within each group', () => {
    triggers.value = {
      status: 'loaded',
      data: [
        makeTrigger({ id: 'live-z', name: 'Zebra' }),
        makeTrigger({ id: 'live-a', name: 'Apple' }),
      ],
    };
    historicalTriggers.value = {
      status: 'loaded',
      data: [
        { id: 'gone-z', name: 'Zebra Old', last_activity: '2026-03-01T00:00:00Z' },
        { id: 'gone-a', name: 'Apple Old', last_activity: '2026-04-15T00:00:00Z' },
      ],
    };
    threadMap.value = new Map();

    expect(triggerFilterOptions.value).toEqual([
      { id: 'live-a', label: 'Apple', deleted: false },
      { id: 'live-z', label: 'Zebra', deleted: false },
      { id: 'gone-a', label: 'Apple Old', deleted: true, lastActivity: '2026-04-15T00:00:00Z' },
      { id: 'gone-z', label: 'Zebra Old', deleted: true, lastActivity: '2026-03-01T00:00:00Z' },
    ]);
  });

  it('keeps same-named live and deleted entries as separate options (no merging)', () => {
    // Delete-and-recreate produces a live trigger and a deleted entry that
    // share a name. They stay distinct in the list — the deleted one's
    // `(until <date>)` suffix lets the user tell them apart in the dropdown.
    triggers.value = {
      status: 'loaded',
      data: [makeTrigger({ id: 'new-uuid', name: 'Nightly Build' })],
    };
    historicalTriggers.value = {
      status: 'loaded',
      data: [{ id: 'old-uuid', name: 'Nightly Build', last_activity: '2026-04-25T00:00:00Z' }],
    };
    threadMap.value = new Map();

    expect(triggerFilterOptions.value).toEqual([
      { id: 'new-uuid', label: 'Nightly Build', deleted: false },
      { id: 'old-uuid', label: 'Nightly Build', deleted: true, lastActivity: '2026-04-25T00:00:00Z' },
    ]);
  });
});

describe('triggerFilterOptions — include-deleted toggle', () => {
  beforeEach(() => {
    triggers.value = { status: 'loaded', data: [makeTrigger({ id: 'live-1', name: 'Live One' })] };
    historicalTriggers.value = {
      status: 'loaded',
      data: [{ id: 'gone-1', name: 'Old Trigger', last_activity: '2026-04-15T00:00:00Z' }],
    };
    threadMap.value = new Map();
    selectedTriggerIds.value = new Set();
  });

  it('excludes deleted triggers when the toggle is off (default)', () => {
    includeDeletedFilterOptions.value = false;
    expect(triggerFilterOptions.value.map(o => o.id)).toEqual(['live-1']);
  });

  it('includes deleted triggers when the toggle is on', () => {
    includeDeletedFilterOptions.value = true;
    expect(triggerFilterOptions.value.map(o => o.id)).toEqual(['live-1', 'gone-1']);
  });

  it('keeps a selected deleted trigger visible even when the toggle is off (stays clearable)', () => {
    includeDeletedFilterOptions.value = false;
    selectedTriggerIds.value = new Set(['gone-1']);
    expect(triggerFilterOptions.value.map(o => o.id)).toEqual(['live-1', 'gone-1']);
  });
});
