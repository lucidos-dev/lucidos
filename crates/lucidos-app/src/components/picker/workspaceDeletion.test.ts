import { beforeEach, describe, expect, it, vi } from 'vitest';
import { confirmCleanup, freezeWorkspaceDeletion } from '../../api/client/control';
import { showConfirm, showToast } from '../../store/store';
import { deleteWorkspaces } from './workspaceDeletion';

vi.mock('../../api/client/control', () => ({
  freezeWorkspaceDeletion: vi.fn(),
  confirmCleanup: vi.fn(),
}));
vi.mock('../../store/store', () => ({ showConfirm: vi.fn(), showToast: vi.fn() }));

const batch = {
  id: 'batch-example',
  phrase: 'Delete Example Workspace',
  members: [{ id: 'example', name: 'Example Workspace', database_backup: true }],
  results: [],
};
const refresh = vi.fn().mockResolvedValue(undefined);

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(freezeWorkspaceDeletion).mockResolvedValue(batch);
  vi.mocked(showConfirm).mockResolvedValue(true);
  vi.mocked(confirmCleanup).mockResolvedValue({
    ...batch, results: [{ id: 'example', status: 'deleted', detail: null }],
  });
});

describe('picker workspace deletion', () => {
  it('names the frozen workspace and echoes the server phrase after confirmation', async () => {
    await deleteWorkspaces(['example'], refresh);
    expect(freezeWorkspaceDeletion).toHaveBeenCalledExactlyOnceWith(['example']);
    expect(showConfirm).toHaveBeenCalledExactlyOnceWith(
      'Delete workspace "Example Workspace"?', 'Delete', { variant: 'danger' },
    );
    expect(confirmCleanup).toHaveBeenCalledExactlyOnceWith(batch.id, batch.phrase);
    expect(showToast).toHaveBeenCalledWith('Deleted "Example Workspace".', 'success');
    expect(refresh).toHaveBeenCalledOnce();
  });

  it('lists exactly the selected workspaces and reports a partial deletion', async () => {
    vi.mocked(freezeWorkspaceDeletion).mockResolvedValue({
      ...batch,
      members: [
        ...batch.members,
        { id: 'scratch', name: 'Scratch', database_backup: false },
      ],
    });
    vi.mocked(confirmCleanup).mockResolvedValue({
      ...batch,
      results: [
        { id: 'example', status: 'deleted', detail: null },
        { id: 'scratch', status: 'skipped', detail: 'Workspace changed' },
      ],
    });

    await deleteWorkspaces(['example', 'scratch'], refresh);

    expect(freezeWorkspaceDeletion).toHaveBeenCalledExactlyOnceWith(['example', 'scratch']);
    expect(showConfirm).toHaveBeenCalledExactlyOnceWith('Delete 2 workspaces?', 'Delete', {
      variant: 'danger',
      details: { groups: [
        { header: 'Move to Trash', items: ['Example Workspace', 'Scratch'] },
        { header: 'Databases will be permanently deleted', items: ['Scratch'] },
      ] },
    });
    expect(showToast).toHaveBeenCalledWith('Deleted "Example Workspace".', 'success');
    expect(showToast).toHaveBeenCalledWith('Could not delete "Scratch": Workspace changed', 'error');
    expect(refresh).toHaveBeenCalledOnce();
  });

  it('warns before deleting one workspace without a database backup', async () => {
    vi.mocked(freezeWorkspaceDeletion).mockResolvedValue({
      ...batch, members: [{ ...batch.members[0], database_backup: false }],
    });
    vi.mocked(showConfirm).mockResolvedValue(false);
    await deleteWorkspaces(['example'], refresh);
    expect(showConfirm).toHaveBeenCalledWith('Delete workspace "Example Workspace"?', 'Delete', {
      variant: 'danger',
      details: { groups: [], intro: 'This workspace’s database will be permanently deleted.' },
    });
    expect(confirmCleanup).not.toHaveBeenCalled();
  });

  it('refuses a batch with duplicate members in place of selected workspaces', async () => {
    vi.mocked(freezeWorkspaceDeletion).mockResolvedValue({
      ...batch, members: [...batch.members, ...batch.members],
    });
    await deleteWorkspaces(['example', 'scratch'], refresh);
    expect(showConfirm).not.toHaveBeenCalled();
    expect(confirmCleanup).not.toHaveBeenCalled();
  });

  it('leaves the workspace intact when the dialog is canceled', async () => {
    vi.mocked(showConfirm).mockResolvedValue(false);
    await deleteWorkspaces(['example'], refresh);
    expect(confirmCleanup).not.toHaveBeenCalled();
    expect(refresh).not.toHaveBeenCalled();
  });

  it('refuses a different frozen workspace', async () => {
    vi.mocked(freezeWorkspaceDeletion).mockResolvedValue({
      ...batch, members: [{ id: 'other', name: 'Other', database_backup: true }],
    });
    await deleteWorkspaces(['example'], refresh);
    expect(showConfirm).not.toHaveBeenCalled();
    expect(confirmCleanup).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('could not verify the selected workspaces'), 'error');
  });

  it('surfaces a freeze failure and leaves the dialog closed', async () => {
    vi.mocked(freezeWorkspaceDeletion).mockRejectedValue(new Error('Database unavailable'));
    await deleteWorkspaces(['example'], refresh);
    expect(showConfirm).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Database unavailable'), 'error');
  });

  it('surfaces a confirm failure and refreshes to show any partial effects', async () => {
    vi.mocked(confirmCleanup).mockRejectedValue(new Error('Cleanup interrupted'));
    await deleteWorkspaces(['example'], refresh);
    expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Cleanup interrupted'), 'error');
    expect(refresh).toHaveBeenCalledOnce();
  });

  it('reports why the gateway skipped the workspace', async () => {
    vi.mocked(confirmCleanup).mockResolvedValue({
      ...batch, results: [{ id: 'example', status: 'skipped', detail: 'Workspace changed' }],
    });
    await deleteWorkspaces(['example'], refresh);
    expect(showToast).toHaveBeenCalledWith('Could not delete "Example Workspace": Workspace changed', 'error');
    expect(refresh).toHaveBeenCalledOnce();
  });
});
