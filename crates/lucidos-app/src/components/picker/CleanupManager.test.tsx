// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import * as api from '../../api/client/control';
import { CleanupManager } from './CleanupManager';
vi.mock('../../api/client/control', () => ({
  getCleanupReview: vi.fn(), listTrash: vi.fn(), freezeCleanup: vi.fn(), confirmCleanup: vi.fn(),
  freezeEmptyTrash: vi.fn(), confirmEmptyTrash: vi.fn(), restoreTrash: vi.fn(), abandonTrashStaging: vi.fn(),
}));
let host: HTMLDivElement;
const row = (origin: 'eval' | 'user'): api.CleanupRow => ({
  id: origin, workspace: { id: origin, name: `${origin} workspace`, dir: '/sample', port: 9000,
    autostart: false, origin, ephemeral: false, expires_at: null, last_opened_at: null },
  directory: '/sample', state: 'registered', folder_bytes: 1024, folder_modified: '2020-01-01T00:00:00Z',
  database: { name: 'sample', bytes: 1024, last_event: null, warning: null },
  running: false, fingerprint: `fingerprint-${origin}`, warning: null,
});
async function click(text: string) {
  const button = await vi.waitFor(() => {
    const found = [...host.querySelectorAll('button')].find(candidate => candidate.textContent === text);
    expect(found?.disabled).toBe(false);
    return found!;
  });
  await act(async () => { button.click(); });
}
beforeEach(() => {
  vi.resetAllMocks(); localStorage.clear();
  host = document.createElement('div'); document.body.append(host);
  vi.mocked(api.getCleanupReview).mockResolvedValue({ scanned_at: 'today', refreshing: false,
    rows: [row('eval'), row('user')], searched_parents: [], registered_databases: [], warnings: [] });
  vi.mocked(api.listTrash).mockResolvedValue([]);
});
afterEach(() => { vi.useRealTimers(); render(null, host); host.remove(); });
it('freezes only suggested harness members and sends the exact frozen phrase', async () => {
  vi.mocked(api.freezeCleanup).mockResolvedValue({ id: 'batch', phrase: 'server confirmation',
    members: [{ id: 'eval', fingerprint: 'fingerprint-eval', workspace: row('eval').workspace!, retained_branches: ['saved'] }], results: [] });
  vi.mocked(api.confirmCleanup).mockResolvedValue({ id: 'batch', phrase: 'server confirmation', members: [], results: [{ id: 'eval', status: 'deleted', detail: null }] });
  const changed = vi.fn().mockResolvedValue(undefined);
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={changed} />, host); });
  await click('Manage workspaces');
  expect(host.querySelectorAll('input[type=checkbox]')).toHaveLength(1);
  await click('Review and delete');
  await vi.waitFor(() => expect(api.freezeCleanup).toHaveBeenCalledWith([{ id: 'eval', fingerprint: 'fingerprint-eval' }]));
  await vi.waitFor(() => expect(host.textContent).toContain('Retained branches: saved'));
  const input = host.querySelector<HTMLInputElement>('[aria-label="Cleanup confirmation"]')!;
  await act(async () => { input.value = 'server confirmation'; input.dispatchEvent(new Event('input', { bubbles: true })); });
  await click('Confirm deletion');
  expect(api.confirmCleanup).toHaveBeenCalledWith('batch', 'server confirmation');
  await vi.waitFor(() => expect(changed).toHaveBeenCalledOnce()); expect(host.textContent).toContain('deleted');
});
it('routes a requested protected workspace into an individual frozen review', async () => {
  vi.mocked(api.freezeCleanup).mockResolvedValue({ id: 'batch', phrase: 'Delete user workspace', members: [], results: [] });
  const handled = vi.fn();
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace="user" onRequestHandled={handled} onChanged={async () => {}} />, host); });
  await vi.waitFor(() => expect(api.freezeCleanup).toHaveBeenCalledWith([{ id: 'user', fingerprint: 'fingerprint-user' }]));
  expect(host.textContent).toContain('Delete user workspace');
  expect(handled).toHaveBeenCalledOnce();
});
it('surfaces failed reads without presenting an empty review', async () => {
  vi.mocked(api.getCleanupReview).mockRejectedValue(new Error('Inventory refused'));
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await click('Manage workspaces');
  expect(host.textContent).toContain('Inventory refused');
  expect(host.textContent).not.toContain('0 resources');
});
it('freezes Empty Trash with named sizes and confirms the exact server phrase', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([{ id: 'archive', name: 'Archived workspace', status: 'deleted', bytes: 4096, restore_available: false, warning: 'No database backup' }]);
  vi.mocked(api.freezeEmptyTrash).mockResolvedValue({ id: 'empty', phrase: 'Empty frozen Trash', members: [{ id: 'archive', bytes: 4096 }], results: [] });
  vi.mocked(api.confirmEmptyTrash).mockResolvedValue({ id: 'empty', phrase: 'Empty frozen Trash', members: [], results: [{ id: 'archive', status: 'failed', detail: 'Archive changed' }] });
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await click('Manage workspaces');
  expect(host.textContent).toContain('No database backup');
  expect([...host.querySelectorAll('button')].find(button => button.textContent === 'Review restore')?.disabled).toBe(true);
  await click('Empty Trash');
  expect(host.textContent).toContain('Total: 4 KB');
  const input = host.querySelector<HTMLInputElement>('[aria-label="Cleanup confirmation"]')!;
  await act(async () => { input.value = 'Empty frozen Trash'; input.dispatchEvent(new Event('input', { bubbles: true })); });
  await click('Confirm Empty Trash');
  expect(api.confirmEmptyTrash).toHaveBeenCalledWith('empty', 'Empty frozen Trash');
  expect(host.textContent).toContain('Archived workspace: failed: Archive changed');
});
it('shows detached recovery and retained-database reset warnings before acting', async () => {
  vi.mocked(api.listTrash).mockResolvedValue([{ id: 'archive', name: 'Interrupted workspace', status: 'restoring', bytes: null, restore_available: true, warning: 'Detached recovery' }]);
  vi.mocked(api.restoreTrash).mockResolvedValue(undefined);
  vi.mocked(api.abandonTrashStaging).mockResolvedValue(undefined);
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await click('Manage workspaces');
  await click('Review restore');
  expect(api.restoreTrash).not.toHaveBeenCalled();
  expect(host.textContent).toContain('Explicitly reattach');
  await click('Restore with detached worktrees');
  expect(api.restoreTrash).toHaveBeenCalledWith('archive');
  await vi.waitFor(() => expect(host.textContent).not.toContain('Restore: Interrupted workspace'));
  await click('Review staging reset');
  expect(host.textContent).toContain('old staging database is retained');
  expect(api.abandonTrashStaging).not.toHaveBeenCalled();
  await click('Reset staging, keep database');
  expect(api.abandonTrashStaging).toHaveBeenCalledWith('archive');
});

it('refreshes an aged selection and refuses changed fingerprints before freezing', async () => {
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await click('Manage workspaces');
  const priorCalls = vi.mocked(api.getCleanupReview).mock.calls.length;
  vi.mocked(api.getCleanupReview).mockResolvedValue({ scanned_at: 'later', refreshing: false,
    rows: [{ ...row('eval'), fingerprint: 'changed' }, row('user')],
    searched_parents: [], registered_databases: [], warnings: [] });
  await click('Review and delete');
  expect(api.getCleanupReview).toHaveBeenCalledTimes(priorCalls + 1);
  expect(api.freezeCleanup).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(host.textContent).toContain('Resources changed'));
  expect(api.freezeCleanup).not.toHaveBeenCalled();
});

it('keeps a requested deletion pending while an expired review is refreshed', async () => {
  vi.useFakeTimers({ toFake: ['Date'] });
  vi.setSystemTime(new Date('2026-01-01T00:00:00Z'));
  const handled = vi.fn();
  const changed = async () => {};
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={handled} onChanged={changed} />, host); });
  vi.setSystemTime(new Date('2026-01-01T00:01:01Z'));
  const fresh: api.CleanupReview = { scanned_at: new Date().toISOString(), refreshing: false,
    rows: [row('eval'), row('user')], searched_parents: [], registered_databases: [], warnings: [] };
  vi.mocked(api.getCleanupReview).mockResolvedValueOnce({ ...fresh, refreshing: true }).mockResolvedValue(fresh);
  vi.mocked(api.freezeCleanup).mockResolvedValue({ id: 'batch', phrase: 'Delete user workspace', members: [], results: [] });
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace="user" onRequestHandled={handled} onChanged={changed} />, host); });
  expect(api.freezeCleanup).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(host.textContent).toContain('Sizes are being refreshed'));
  expect(api.freezeCleanup).not.toHaveBeenCalled();
  await act(async () => { render(<CleanupManager tick={1} requestedWorkspace="user" onRequestHandled={handled} onChanged={changed} />, host); });
  await vi.waitFor(() => expect(api.freezeCleanup).toHaveBeenCalledWith([{ id: 'user', fingerprint: 'fingerprint-user' }]));
  expect(handled).toHaveBeenCalledOnce();
});

it('stays hidden when only ordinary workspaces exist and Trash is empty', async () => {
  const legacy = { ...row('user'), id: 'legacy', workspace: { ...row('user').workspace!, id: 'legacy', origin: 'unknown' as const } };
  vi.mocked(api.getCleanupReview).mockResolvedValue({ scanned_at: 'today', refreshing: false,
    rows: [row('user'), legacy], searched_parents: [], registered_databases: [], warnings: [] });
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await vi.waitFor(() => expect(api.listTrash).toHaveBeenCalled());
  await act(async () => {});
  expect(host.textContent).not.toContain('Manage workspaces');
});

it('appears on a later gateway poll once cleanup work exists', async () => {
  const review = (rows: api.CleanupRow[]): api.CleanupReview =>
    ({ scanned_at: 'today', refreshing: false, rows, searched_parents: [], registered_databases: [], warnings: [] });
  vi.mocked(api.getCleanupReview).mockResolvedValue(review([row('user')]));
  const props = { requestedWorkspace: null, onRequestHandled: () => {}, onChanged: async () => {} };
  await act(async () => { render(<CleanupManager tick={0} {...props} />, host); });
  await vi.waitFor(() => expect(api.listTrash).toHaveBeenCalled());
  await act(async () => {});
  expect(host.textContent).not.toContain('Manage workspaces');
  vi.mocked(api.getCleanupReview).mockResolvedValue(review([row('user'), row('eval')]));
  await act(async () => { render(<CleanupManager tick={1} {...props} />, host); });
  await vi.waitFor(() => expect(host.textContent).toContain('Manage workspaces'));
});

it('opens for a requested deletion even when nothing else needs cleanup', async () => {
  vi.mocked(api.getCleanupReview).mockResolvedValue({ scanned_at: 'today', refreshing: false,
    rows: [row('user')], searched_parents: [], registered_databases: [], warnings: [] });
  vi.mocked(api.freezeCleanup).mockResolvedValue({ id: 'batch', phrase: 'Delete user workspace', members: [], results: [] });
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace="user" onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await vi.waitFor(() => expect(host.textContent).toContain('Delete user workspace'));
});

it('requires individual review for unknown folders and databases', async () => {
  const folder = { ...row('user'), id: 'folder', workspace: null, state: 'not-registered' };
  const database = { ...folder, id: 'database', state: 'database-only' };
  vi.mocked(api.getCleanupReview).mockResolvedValue({ scanned_at: 'today', refreshing: false,
    rows: [folder, database], searched_parents: [], registered_databases: [], warnings: [] });
  await act(async () => { render(<CleanupManager tick={0} requestedWorkspace={null} onRequestHandled={() => {}} onChanged={async () => {}} />, host); });
  await click('Manage workspaces');
  expect(host.querySelectorAll('input[type=checkbox]')).toHaveLength(0);
  expect([...host.querySelectorAll('button')].filter(button => button.textContent === 'Review deletion')).toHaveLength(2);
});
