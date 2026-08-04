import { triggerGroups, showToast, showConfirm } from '../store';
import type { TriggerGroup } from '../types';
import { toFailed, setLoadingIfFresh } from '../types';
import {
  listTriggerGroups,
  createTriggerGroup as apiCreate,
  updateTriggerGroup as apiUpdate,
  deleteTriggerGroup as apiDelete,
  reorderTriggerGroups as apiReorder,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';

/** Fetch the trigger-group registry from the engine. Called from `useStartup`
 *  and re-fetched after group mutations to keep the panel in sync. */
export async function loadTriggerGroups(): Promise<void> {
  setLoadingIfFresh(triggerGroups);
  try {
    const data = await listTriggerGroups();
    triggerGroups.value = { status: 'loaded', data: data.groups || [] };
  } catch (error) {
    triggerGroups.value = toFailed(error);
  }
}

/** Patch one group into the in-memory registry. Used by the mutation helpers
 *  below, which get the freshly-created/updated group back from the HTTP
 *  handler before the SSE event round-trips, so the panel doesn't wait a
 *  round-trip to show the change. The `TriggerGroup*` SSE arms do NOT come
 *  through here: `entityReferences.ts` re-fetches the whole registry with
 *  `loadTriggerGroups()` instead, since a group event can move member counts
 *  on rows it doesn't name. */
export function upsertTriggerGroup(group: TriggerGroup): void {
  const current = triggerGroups.value;
  if (current.status !== 'loaded') return;
  const next = [...current.data];
  const idx = next.findIndex(g => g.id === group.id);
  if (idx >= 0) next[idx] = { ...next[idx], ...group };
  else next.push(group);
  // Always keep panel order stable (asc).
  next.sort((a, b) => a.order - b.order || a.created.localeCompare(b.created));
  triggerGroups.value = { status: 'loaded', data: next };
}

/** Remove one group from the in-memory registry. */
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
  if (!(await showConfirm(`Delete group "${name}"?`))) return;
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
    // Optimistically apply locally so the panel doesn't flicker waiting for SSE.
    const current = triggerGroups.value;
    if (current.status === 'loaded') {
      const byId = new Map(ordering.map(e => [e.id, e.order]));
      const next = current.data.map(g =>
        byId.has(g.id) ? { ...g, order: byId.get(g.id)! } : g,
      );
      next.sort((a, b) => a.order - b.order || a.created.localeCompare(b.created));
      triggerGroups.value = { status: 'loaded', data: next };
    }
  } catch (error) {
    showToast('Failed to reorder groups: ' + errorDetail(error), 'error');
  }
}
