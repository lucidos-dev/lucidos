import type { Tap } from '@lucidos/sdk';
import { API, WORKSPACE_ID, parseWorkspaceRoute } from '../../utils/basePath';
import { ApiError, mutatingFetch, requestGenerationGuard, throwIfNotOk } from '../../api/client/_core';
import { errorDetail } from '../../utils/errorDetail';
import { showConfirm, showToast } from '../store';

export function restoreFailureMessage(error: unknown): string {
  const answered = error instanceof ApiError && !error.bootSplash
    && !error.reason.includes('snapshot_restore_delivery_unknown');
  return answered ? errorDetail(error) : 'Restore delivery may be unknown. Check restore status before trying again.';
}

export async function confirmSnapshotRestore(tap: Extract<Tap, { kind: 'restore-snapshot' }>): Promise<void> {
  const checkGeneration = requestGenerationGuard();
  let submitted = false;
  try {
    const workspace = WORKSPACE_ID && parseWorkspaceRoute(WORKSPACE_ID);
    if (!workspace || workspace.slug !== tap.workspace_id) {
      throw new Error('Open the notification in its workspace before undoing the restore');
    }
    if (!(await showConfirm(
      `Restore workspace "${workspace.slug}" from ${tap.handle}? Current workspace data will be replaced. A recovery snapshot will be kept. Sent messages, remote pushes and other external side effects are not undone.`,
      'Undo restore',
      { title: 'Undo restore', variant: 'danger' },
    ))) return;
    checkGeneration();
    submitted = true;
    const response = await mutatingFetch(`${API}/snapshots/${encodeURIComponent(tap.handle)}/restore`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ expected_workspace_id: workspace.slug, confirmed: true }),
    });
    await throwIfNotOk(response);
    showToast('Restore requested', 'success', { title: 'Undo restore' });
  } catch (error) {
    const message = submitted
      ? restoreFailureMessage(error)
      : error instanceof Error ? error.message : 'Unable to request restore';
    showToast(message, 'error', { title: 'Undo restore' });
  }
}
