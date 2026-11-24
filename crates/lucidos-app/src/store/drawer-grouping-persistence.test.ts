import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

const GROUPING_KEY = 'lucidos-drawer-grouping';
const SELECTED_GROUP_KEY = 'lucidos-drawer-selected-ongoing-group';

describe('drawer grouping persistence', () => {
  beforeEach(() => {
    vi.resetModules();
    localStorage.removeItem(GROUPING_KEY);
    localStorage.removeItem(SELECTED_GROUP_KEY);
  });

  afterEach(() => {
    localStorage.removeItem(GROUPING_KEY);
    localStorage.removeItem(SELECTED_GROUP_KEY);
  });

  it('restores Folders and Blocked when nothing is stored', async () => {
    const { drawerGrouping, selectedOngoingGroup } = await import('./store');
    expect(drawerGrouping.value).toBe('folders');
    expect(selectedOngoingGroup.value).toBe('blocked');
  });

  it('restores a stored Ongoing grouping and its selected group', async () => {
    localStorage.setItem(GROUPING_KEY, 'ongoing');
    localStorage.setItem(SELECTED_GROUP_KEY, 'review');
    const { drawerGrouping, selectedOngoingGroup } = await import('./store');
    expect(drawerGrouping.value).toBe('ongoing');
    expect(selectedOngoingGroup.value).toBe('review');
  });

  it.each([
    ['an unknown value', 'sections', 'running'],
    ['a retired value', 'status', 'idle'],
    ['the old Blocked name', 'sections', 'attention'],
  ])('restores the defaults from %s', async (_what, grouping, group) => {
    localStorage.setItem(GROUPING_KEY, grouping);
    localStorage.setItem(SELECTED_GROUP_KEY, group);
    const { drawerGrouping, selectedOngoingGroup } = await import('./store');
    expect(drawerGrouping.value).toBe('folders');
    expect(selectedOngoingGroup.value).toBe('blocked');
  });

  it('ignores the retired status filter key', async () => {
    localStorage.setItem('lucidos-alt-view', 'attention');
    const { drawerGrouping } = await import('./store');
    expect(drawerGrouping.value).toBe('folders');
    localStorage.removeItem('lucidos-alt-view');
  });

  it('writes each pick back, clearing the grouping key for Folders', async () => {
    const { setDrawerGrouping, setSelectedOngoingGroup } = await import('./store');
    setDrawerGrouping('ongoing');
    expect(localStorage.getItem(GROUPING_KEY)).toBe('ongoing');
    setDrawerGrouping('folders');
    expect(localStorage.getItem(GROUPING_KEY)).toBeNull();
    setSelectedOngoingGroup('drafts');
    expect(localStorage.getItem(SELECTED_GROUP_KEY)).toBe('drafts');
  });

  it('round-trips a pick through a reload', async () => {
    const first = await import('./store');
    first.setDrawerGrouping('ongoing');
    first.setSelectedOngoingGroup('in-flight');
    vi.resetModules();
    const second = await import('./store');
    expect(second.drawerGrouping.value).toBe('ongoing');
    expect(second.selectedOngoingGroup.value).toBe('in-flight');
  });
});
