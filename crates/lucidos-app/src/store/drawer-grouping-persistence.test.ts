/**
 * The drawer grouping and the Ongoing grouping's selected group survive a
 * reload, per device. `store.ts` restores both from localStorage at init.
 * Their one writer each (`setDrawerGrouping`, `setSelectedOngoingGroup`)
 * writes them back. The grouping's key is cleared for Folders, so a pristine
 * state restores pristine. An unknown stored value restores the default:
 * Folders, and Blocked.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// The stored keys are a contract with every device's localStorage, so the
// test pins them as literals rather than reading them from the store.
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
    // The grouping was once called Status, and it once had an Idle group.
    ['a retired value', 'status', 'idle'],
    // Blocked was once called Needs attention, stored as `attention`.
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
