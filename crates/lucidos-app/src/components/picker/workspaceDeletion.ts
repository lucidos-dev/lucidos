import { confirmCleanup, freezeWorkspaceDeletion } from '../../api/client/control';
import { showConfirm, showToast } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { withProgressToast } from './progressToast';

export async function deleteWorkspaces(ids: string[], onChanged: () => Promise<void>): Promise<void> {
  let confirming = false;
  try {
    const batch = await freezeWorkspaceDeletion(ids);
    const byId = new Map(batch.members.map((member) => [member.id, member]));
    const members = ids.map((id) => byId.get(id)).filter((member) => member !== undefined);
    if (batch.members.length !== ids.length || byId.size !== ids.length || members.length !== ids.length
      || members.some((member) => !member.name || typeof member.database_backup !== 'boolean')) {
      throw new Error('The gateway could not verify the selected workspaces. Refresh the picker and try again.');
    }
    const names = members.map((member) => member.name);
    const withoutBackup = members.filter((member) => !member.database_backup).map((member) => member.name);
    const title = ids.length === 1 ? `Delete workspace "${names[0]}"?` : `Delete ${ids.length} workspaces?`;
    const groups = ids.length > 1 ? [{ header: 'Move to Trash', items: names }] : [];
    if (withoutBackup.length && ids.length > 1) {
      groups.push({ header: 'Databases will be permanently deleted', items: withoutBackup });
    }
    const details = groups.length || withoutBackup.length ? {
      groups,
      ...(ids.length === 1 && { intro: 'This workspace’s database will be permanently deleted.' }),
    } : undefined;
    if (!await showConfirm(title, 'Delete', { variant: 'danger', ...(details && { details }) })) return;
    confirming = true;
    const progress = ids.length === 1 ? `Deleting "${names[0]}"…` : `Deleting ${ids.length} workspaces…`;
    const { results } = await withProgressToast('workspace-delete', progress, confirmCleanup(batch.id, batch.phrase));
    const resultsById = new Map(results.map((result) => [result.id, result]));
    const deleted = [];
    for (const [index, id] of ids.entries()) {
      const name = names[index];
      const result = resultsById.get(id);
      if (result?.status === 'deleted') {
        deleted.push(name);
      } else {
        showToast(`Could not delete "${name}": ${result?.detail ?? 'The gateway did not report a deletion.'}`, 'error');
      }
    }
    if (deleted.length === 1) showToast(`Deleted "${deleted[0]}".`, 'success');
    if (deleted.length > 1) showToast(`Deleted ${deleted.length} workspaces.`, 'success');
  } catch (error) {
    showToast(`Could not delete workspace: ${errorDetail(error)}`, 'error');
  } finally {
    if (confirming) await onChanged();
  }
}
