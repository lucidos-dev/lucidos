import { test, expect } from './fixtures';
import { assertHealthy } from './helpers';
import type { CleanupResult, WorkspaceStatus } from '../src/api/client/control';

test('workspace picker confirms named deletions and restores from Trash', async ({ page }) => {
  await assertHealthy(page);
  const first: WorkspaceStatus = { id: 'example', name: 'Example Workspace', port: 5301, health: 'unhealthy', autostart: false };
  const second: WorkspaceStatus = { id: 'scratch', name: 'Scratch', port: 5302, health: 'unhealthy', autostart: false };
  const unselected: WorkspaceStatus = { id: 'keep', name: 'Keep Me', port: 5303, health: 'unhealthy', autostart: false };
  let workspaces = [first, second, unselected];
  const remote = { gateway: 'example-server', state: 'reachable', host: 'server.example',
    workspaces: [{ id: 'notes', name: 'Remote Notes', port: 5304, health: 'healthy' }] };
  const archive = { id: 'archive', name: first.name, bytes: 2048,
    status: 'deleted', restore_available: true, warning: null };
  const freezes: string[][] = [];
  const mutations: string[] = [];
  let inTrash = false;
  const batchFor = (members: typeof workspaces) => ({
    id: members.length === 1 ? 'single' : 'bulk',
    phrase: members.length === 1 ? 'Delete Example Workspace' : 'Delete Example Workspace and Scratch',
    members: members.map(workspace => ({
      id: workspace.id, name: workspace.name, database_backup: workspace.id !== second.id,
    })),
    results: [],
  });
  const single = batchFor([first]);
  const bulk = batchFor([first, second]);

  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname.startsWith('/~/api/v1/')) {
      const path = url.pathname.slice('/~/api/v1/'.length);
      expect(path).not.toBe('control/cleanup');
      if (path === 'internal/client-log') {
        await route.fulfill({ status: 204 });
        return;
      }
      if (path === 'control/gateway/check-updates') {
        await route.fulfill({ json: { enabled: false, supported: false, current_version: 'test', checked_at: null, last_error: null, latest: null, update_address: null, update_address_error: null } });
        return;
      }
      let body: unknown;
      if (request.method() === 'POST') {
        if (path === 'control/workspace-delete-batches') {
          const { workspaces: ids } = request.postDataJSON();
          freezes.push(ids);
          body = ids.length === 1 ? single : bulk;
        } else {
          mutations.push(path);
          const batch = [single, bulk].find(({ id }) => path === `control/cleanup/batches/${id}/confirm`);
          if (batch) {
            expect(request.postDataJSON()).toEqual({ confirm: batch.phrase });
            workspaces = [second, unselected];
            inTrash = true;
            const results: CleanupResult[] = [{ id: first.id, status: 'deleted', detail: null }];
            if (batch === bulk) results.push({
              id: second.id, status: 'skipped', detail: 'workspace changed since selection; skipped',
            });
            body = { ...batch, results };
          } else {
            expect(path).toBe('control/trash/archive/restore');
            expect(inTrash).toBe(true);
            workspaces = [first, second, unselected];
            inTrash = false;
            await route.fulfill({ status: 204 });
            return;
          }
        }
      } else {
        expect(request.method()).toBe('GET');
        const reads: Record<string, unknown> = {
          'auth/session': { paired: true },
          'control/workspaces': { workspaces },
          'remote-workspaces': [remote],
          'control/restore-status': { status: 'idle' },
          'control/gateway/status': { build_id: 'test', update_available: false },
          'control/trash': inTrash ? [archive] : [],
        };
        body = reads[path] ?? {};
      }
      await route.fulfill({ json: body });
    } else if (request.isNavigationRequest() && url.pathname === '/~/') {
      const response = await route.fetch({ url: new URL('/', url).toString() });
      const html = await response.text();
      expect(html).toContain('<head>');
      const picker = html.replace('<head>', '<head><base href="/~/">')
        .replace(/<meta name="lucidos-(?:workspace-id|gateway-port)"[^>]*>/g, '');
      await route.fulfill({ response, body: picker });
    } else if (url.pathname.startsWith('/~/')) {
      url.pathname = url.pathname.slice(2);
      await route.fulfill({ response: await route.fetch({ url: url.toString() }) });
    } else {
      await route.continue();
    }
  });
  await page.goto('/~/?pick');
  await expect(page.getByRole('button', { name: 'Manage workspaces', exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'More actions for Example Workspace' }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  const singleDialog = page.getByRole('dialog').filter({ hasText: 'Delete workspace "Example Workspace"?' });
  await expect(singleDialog).toBeVisible();
  await singleDialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(singleDialog).toBeHidden();
  await expect(page.getByRole('button', { name: 'More actions for Example Workspace' })).toBeVisible();
  expect(freezes).toEqual([[first.id]]);
  expect(mutations).toEqual([]);

  await page.getByRole('button', { name: 'Select', exact: true }).click();
  await expect(page.getByRole('button', { name: 'More actions for Remote Notes on example-server' })).toBeVisible();
  await expect(page.getByRole('checkbox')).toHaveCount(3);
  await expect(page.getByRole('checkbox', { name: 'Select Remote Notes' })).toHaveCount(0);
  await page.getByRole('checkbox', { name: 'Select Example Workspace' }).check();
  await page.getByRole('checkbox', { name: 'Select Scratch' }).check();
  await page.getByRole('button', { name: 'Delete 2 workspaces' }).click();
  const bulkDialog = page.getByRole('dialog').filter({ hasText: 'Delete 2 workspaces?' });
  await expect(bulkDialog.getByRole('listitem')).toHaveText([first.name, second.name, second.name]);
  await expect(bulkDialog.getByText('Databases will be permanently deleted')).toBeVisible();
  expect(freezes).toEqual([[first.id], [first.id, second.id]]);
  expect(mutations).toEqual([]);
  await bulkDialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(page.getByText('Could not delete "Scratch": workspace changed since selection; skipped')).toBeVisible();
  await expect(page.getByRole('button', { name: 'More actions for Example Workspace' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'More actions for Scratch' })).toBeVisible();

  const trash = page.getByRole('region', { name: 'Trash' });
  await expect(trash.getByText(first.name, { exact: true })).toBeVisible();
  await trash.getByRole('button', { name: 'Restore', exact: true }).click();
  const restoreDialog = page.getByRole('dialog').filter({ hasText: 'Restore "Example Workspace"?' });
  await expect(restoreDialog).toBeVisible();
  expect(mutations).toEqual(['control/cleanup/batches/bulk/confirm']);
  await restoreDialog.getByRole('button', { name: 'Restore', exact: true }).click();
  await expect(page.getByRole('button', { name: 'More actions for Example Workspace' })).toBeVisible();
  await expect(trash).toBeHidden();
  expect(mutations).toEqual(['control/cleanup/batches/bulk/confirm', 'control/trash/archive/restore']);

  await page.getByRole('button', { name: 'More actions for Example Workspace' }).click();
  await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
  await singleDialog.getByRole('button', { name: 'Delete', exact: true }).click();
  await expect(trash.getByText(first.name, { exact: true })).toBeVisible();
  expect(freezes).toEqual([[first.id], [first.id, second.id], [first.id]]);
  expect(mutations.at(-1)).toBe('control/cleanup/batches/single/confirm');
});
