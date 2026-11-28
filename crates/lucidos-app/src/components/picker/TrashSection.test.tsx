// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import * as api from '../../api/client/control';
import { showConfirm, showToast } from '../../store/store';
import { TrashSection, useTrashInventory } from './TrashSection';

vi.mock('../../api/client/control', () => ({
  listTrash: vi.fn(), restoreTrash: vi.fn(), abandonTrashStaging: vi.fn(),
  freezeEmptyTrash: vi.fn(), confirmEmptyTrash: vi.fn(), finishTrashDelete: vi.fn(),
}));
vi.mock('../../store/store', () => ({
  showConfirm: vi.fn(), showToast: vi.fn(), removeToast: vi.fn(), scaledDurationMs: (ms: number) => ms,
}));

const archive: api.TrashEntry = {
  id: 'archive', name: 'Archived workspace', status: 'restoring',
  bytes: 4096, restore_available: true, finish_available: false, database_backup: true, warning: 'Restore interrupted',
};
const interrupted: api.TrashEntry = {
  id: 'workspace-personal', name: 'personal', status: 'prepared',
  bytes: 4096, restore_available: true, finish_available: true, database_backup: true, warning: null,
};
const changed = vi.fn().mockResolvedValue(undefined);
let host: HTMLDivElement;

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.listTrash).mockResolvedValue([]);
  vi.mocked(showConfirm).mockResolvedValue(true);
  host = document.createElement('div');
  document.body.append(host);
});
afterEach(() => {
  render(null, host);
  host.remove();
});

function TrashFixture({ revision }: { revision: number }) {
  const inventory = useTrashInventory(revision, true);
  return <TrashSection inventory={inventory} showSkeleton={false} onChanged={changed} />;
}

async function mount(revision = 0) {
  await act(async () => { render(<TrashFixture revision={revision} />, host); });
}

async function click(label: string) {
  const button = await vi.waitFor(() => {
    const found = [...host.querySelectorAll('button')].find((candidate) => candidate.textContent?.trim() === label);
    expect(found?.disabled).toBe(false);
    return found!;
  });
  await act(async () => { button.click(); });
}

it('hides empty Trash and shows a failed read with Retry', async () => {
  await mount();
  expect(host.querySelector('section')).toBeNull();

  vi.mocked(api.listTrash).mockRejectedValueOnce(new Error('Journal unreadable'));
  await mount(1);
  await vi.waitFor(() => expect(host.textContent).toContain('Journal unreadable'));
  await click('Retry');
  await vi.waitFor(() => expect(host.textContent).toBe(''));
});

it('restores or resets the selected journal only after confirmation', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([archive]);
  await mount();
  vi.mocked(showConfirm).mockResolvedValueOnce(false);
  await click('Restore');
  expect(api.restoreTrash).not.toHaveBeenCalled();

  await click('Restore');
  expect(api.restoreTrash).toHaveBeenCalledWith('archive');
  await click('Reset staging');
  expect(api.abandonTrashStaging).toHaveBeenCalledWith('archive');
});

it('sends the frozen Empty Trash phrase and reports skipped entries', async () => {
  const batch = { id: 'empty', phrase: 'Empty frozen Trash', members: [{ id: 'archive', bytes: 4096 }], results: [] };
  vi.mocked(api.listTrash).mockResolvedValue([archive]);
  vi.mocked(api.freezeEmptyTrash).mockResolvedValue(batch);
  vi.mocked(api.confirmEmptyTrash).mockResolvedValue({
    ...batch, results: [{ id: 'archive', status: 'skipped', detail: 'Archive changed' }],
  });
  await mount();
  await click('Empty Trash');
  expect(showConfirm).toHaveBeenCalledWith(expect.any(String), 'Empty Trash', expect.objectContaining({ variant: 'danger' }));
  expect(api.confirmEmptyTrash).toHaveBeenCalledWith(batch.id, batch.phrase);
  await vi.waitFor(() => expect(showToast).toHaveBeenCalledWith(expect.stringContaining('Archive changed'), 'error'));
});

it('finishes an interrupted delete only after a danger confirmation and names the result', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([interrupted]);
  await mount();
  vi.mocked(showConfirm).mockResolvedValueOnce(false);
  await click('Finish delete');
  expect(api.finishTrashDelete).not.toHaveBeenCalled();
  expect(showConfirm).toHaveBeenLastCalledWith(expect.any(String), 'Finish delete', expect.objectContaining({ variant: 'danger' }));

  await click('Finish delete');
  expect(api.finishTrashDelete).toHaveBeenCalledWith('workspace-personal');
  await vi.waitFor(() => expect(showToast).toHaveBeenCalledWith('Deleted "personal".', 'success'));

  vi.mocked(api.finishTrashDelete).mockRejectedValueOnce(new Error('could not copy the database, so nothing was dropped'));
  await click('Finish delete');
  await vi.waitFor(() => expect(showToast).toHaveBeenCalledWith('could not copy the database, so nothing was dropped', 'error'));
});

it('says Finish delete keeps no database copy when the workspace has no backup', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([{ ...interrupted, database_backup: false }]);
  await mount();
  vi.mocked(showConfirm).mockResolvedValueOnce(false);
  await click('Finish delete');
  expect(showConfirm).toHaveBeenLastCalledWith(expect.stringContaining('no copy'), 'Finish delete', expect.anything());
});

it('says Finish delete keeps a database copy when the workspace has one', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([interrupted]);
  await mount();
  vi.mocked(showConfirm).mockResolvedValueOnce(false);
  await click('Finish delete');
  expect(showConfirm).toHaveBeenLastCalledWith(expect.stringContaining('keeps a copy'), 'Finish delete', expect.anything());
});

it('offers Finish delete only for an interrupted delete', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([archive]);
  await mount();
  await vi.waitFor(() => expect(host.textContent).toContain('Archived workspace'));
  expect(host.textContent).not.toContain('Finish delete');
});
