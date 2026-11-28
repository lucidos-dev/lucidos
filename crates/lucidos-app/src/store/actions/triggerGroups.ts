import { triggerGroups, showToast, showConfirm } from '../store';
import type { TriggerGroup } from '../types';
import { toFailed, setLoadingIfFresh } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import {
  listTriggerGroups,
  createTriggerGroup as apiCreate,
  updateTriggerGroup as apiUpdate,
  deleteTriggerGroup as apiDelete,
  reorderTriggerGroups as apiReorder,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { instantMicros } from '../../utils/isoInstant';

function byPanelOrder(a: TriggerGroup, b: TriggerGroup): number {
  return a.order - b.order || (instantMicros(a.created) ?? 0) - (instantMicros(b.created) ?? 0);
}

export const triggerGroupsSlice: SliceOf<'trigger_groups'> = {
  begin: () => setLoadingIfFresh(triggerGroups),
  apply: (data) => { triggerGroups.value = { status: 'loaded', data: data.groups || [] }; },
  fail: (error) => { triggerGroups.value = toFailed(error); },
};

export function loadTriggerGroups(): Promise<void> {
  return loadSlice('trigger_groups', triggerGroupsSlice, () => listTriggerGroups());
}

export function upsertTriggerGroup(group: TriggerGroup): void {
  const current = triggerGroups.value;
  if (current.status !== 'loaded') return;
  const next = [...current.data];
  const idx = next.findIndex(g => g.id === group.id);
  if (idx >= 0) next[idx] = { ...next[idx], ...group };
  else next.push(group);
  next.sort(byPanelOrder);
  triggerGroups.value = { status: 'loaded', data: next };
}

export function removeTriggerGroup(groupId: string): void {
  const current = triggerGroups.value;
  if (current.status !== 'loaded') return;
  triggerGroups.value = {
    status: 'loaded',
    data: current.data.filter(g => g.id !== groupId),
  };
}

export async function createTriggerGroup(name: string): Promise<TriggerGroup | null> {
  try {
    const group = await apiCreate({ name });
    upsertTriggerGroup(group);
    return group;
  } catch (error) {
    showToast('Failed to create group: ' + errorDetail(error), 'error');
    return null;
  }
}

export async function renameTriggerGroup(id: string, name: string): Promise<boolean> {
  try {
    const group = await apiUpdate(id, { name });
    upsertTriggerGroup(group);
    return true;
  } catch (error) {
    showToast('Failed to rename group: ' + errorDetail(error), 'error');
    return false;
  }
}

export async function deleteTriggerGroup(id: string, name: string): Promise<void> {
  if (!(await showConfirm(`Delete group "${name}"?`, 'Delete', { variant: 'danger' }))) return;
  try {
    const conflict = await apiDelete(id);
    if (conflict) {
      const count = conflict.member_count;
      showToast(
        `Move or delete the ${count} trigger${count === 1 ? '' : 's'} in "${name}" first.`,
        'error',
      );
      return;
    }
    removeTriggerGroup(id);
  } catch (error) {
    showToast('Failed to delete group: ' + errorDetail(error), 'error');
  }
}

export async function reorderTriggerGroups(
  ordering: Array<{ id: string; order: number }>,
): Promise<void> {
  try {
    await apiReorder(ordering);
    const current = triggerGroups.value;
    if (current.status === 'loaded') {
      const byId = new Map(ordering.map(e => [e.id, e.order]));
      const next = current.data.map(g =>
        byId.has(g.id) ? { ...g, order: byId.get(g.id)! } : g,
      );
      next.sort(byPanelOrder);
      triggerGroups.value = { status: 'loaded', data: next };
    }
  } catch (error) {
    showToast('Failed to reorder groups: ' + errorDetail(error), 'error');
  }
}
