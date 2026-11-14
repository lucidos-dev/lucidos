import { test, expect } from './fixtures';
import { assertHealthy } from './helpers';
import type { CleanupRow } from '../src/api/client/control';

// The real picker bundle, with an isolated control plane: no request in this
// flow can delete a workspace or reach the machine's gateway.
test('workspace cleanup confirms frozen members and exposes partial results', async ({ page }, testInfo) => {
  await assertHealthy(page);
  const row = (id: string, origin: 'eval' | 'user'): CleanupRow => ({
    id, workspace: { id, name: id, dir: `/sample/${id}`, port: 5300, autostart: false,
      origin, ephemeral: origin === 'eval', expires_at: null, last_opened_at: null },
    directory: `/sample/${id}`, state: 'registered', folder_bytes: 1024,
    folder_modified: '2020-01-01T00:00:00Z', database: {
      name: id, bytes: 1024, last_event: null, warning: null,
    }, running: false, fingerprint: `observed-${id}`, warning: null,
  });
  const original = [row('eval-one', 'eval'), row('eval-two', 'eval'), row('user-workspace', 'user')];
  let rows = original;
  let frozen = false;
  let confirmed = false;
  const phrase = 'Delete the two frozen eval workspaces';
  const batch = {
    id: 'browser-batch', phrase,
    members: original.slice(0, 2).map(member => ({ id: member.id,
      fingerprint: member.fingerprint, workspace: member.workspace!, retained_branches: ['saved-work'] })),
    results: [] as { id: string; status: string; detail: string | null }[],
  };
  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.pathname.startsWith('/~/api/v1/')) {
      const path = url.pathname.slice('/~/api/v1/'.length);
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
        // The picker is authenticated by its paired gateway cookie. A device-id
        // header alone authorizes nothing (covered by gateway auth tests).
        if (path === 'control/cleanup/batches') {
          expect(request.postDataJSON()).toEqual({ members: original.slice(0, 2)
            .map(member => ({ id: member.id, fingerprint: member.fingerprint })) });
          frozen = true;
          rows = [...original, row('eval-arrived-later', 'eval')];
          body = batch;
        } else {
          expect(path).toBe('control/cleanup/batches/browser-batch/confirm');
          expect(request.postDataJSON()).toEqual({ confirm: phrase });
          confirmed = true;
          rows = rows.filter(member => member.id !== 'eval-one');
          body = { ...batch, results: [
            { id: 'eval-one', status: 'deleted', detail: null },
            { id: 'eval-two', status: 'skipped', detail: 'Workspace changed after review' },
          ] };
        }
      } else {
        expect(request.method()).toBe('GET');
        const reads: Record<string, unknown> = {
          'auth/session': { paired: true },
          'control/workspaces': { workspaces: [] },
          'remote-workspaces': [],
          'control/restore-status': { status: 'idle' },
          'control/gateway/status': { build_id: 'test', update_available: false },
          'control/cleanup': { scanned_at: '2026-01-01T00:00:00Z', refreshing: false,
            rows, searched_parents: [], registered_databases: [], warnings: [] },
          'control/trash': confirmed ? [{ id: 'archive', name: 'eval-one', bytes: 2048,
            status: 'deleted', restore_available: false, warning: 'No database backup' }] : [],
        };
        body = reads[path] ?? {};
      }
      await route.fulfill({ json: body });
    } else if (request.isNavigationRequest() && url.pathname === '/~/') {
      const response = await route.fetch({ url: new URL('/', url).toString() });
      const html = await response.text();
      // A direct engine has no base element. Insert before any comments that
      // document base syntax, matching the gateway's actual picker stamp.
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
  await page.getByRole('button', { name: 'Manage workspaces', exact: true }).click();
  await expect(page.getByRole('checkbox')).toHaveCount(2);
  await expect(page.getByRole('checkbox').first()).toBeChecked();
  await page.getByRole('button', { name: 'Review and delete', exact: true }).click();
  await expect(page.getByText('Retained branches: saved-work').first()).toBeVisible();
  expect(frozen).toBe(true);
  await page.getByRole('heading', { name: 'Confirm cleanup', exact: true }).scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('cleanup-confirmation.png'), fullPage: true });
  await page.getByRole('button', { name: 'Refresh review', exact: true }).click();
  await expect(page.getByText('eval-arrived-later', { exact: true })).toBeVisible();
  const confirm = page.getByRole('button', { name: 'Confirm deletion', exact: true });
  await expect(confirm).toBeDisabled();
  await page.getByLabel('Cleanup confirmation').fill('incorrect phrase');
  await expect(confirm).toBeDisabled();
  await page.getByLabel('Cleanup confirmation').fill(phrase);
  await confirm.click();
  await expect(page.getByText('eval-one: deleted', { exact: true })).toBeVisible();
  await expect(page.getByText(/eval-two: skipped/)).toContainText('Workspace changed after review');
  await expect(page.getByText('No database backup', { exact: true })).toBeVisible();
  await expect(page.getByRole('checkbox', { name: 'Select eval-one', exact: true })).toHaveCount(0);
  expect(confirmed).toBe(true);
  await page.screenshot({ path: testInfo.outputPath('cleanup-results.png'), fullPage: true });
});
