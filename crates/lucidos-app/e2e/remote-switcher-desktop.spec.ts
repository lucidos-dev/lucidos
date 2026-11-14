import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp } from './helpers';

test('opens a remote workspace through this gateway while local rows stay available', async ({ page }) => {
  await assertHealthy(page);
  // Stamp a gateway context over the disposable engine; relay local requests
  // back to it while the remote workspace uses the stub below.
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.isNavigationRequest() && url.pathname === '/') {
      const response = await route.fetch();
      const html = (await response.text()).replace(/<base\s[^>]*>/g, '');
      await route.fulfill({ response, body: html.replace('<head>', '<head><base href="/dev/">') });
    } else if (url.pathname.startsWith('/dev/')) {
      url.pathname = url.pathname.slice('/dev'.length);
      await route.fulfill({ response: await route.fetch({ url: url.toString() }) });
    } else await route.fallback();
  });
  await page.route('**/~/api/v1/control/workspaces', (route) => route.fulfill({ json: { workspaces: [
    { id: 'dev', name: 'Local workspace', health: 'healthy', autostart: false },
  ] } }));
  await page.route('**/~/api/v1/remote-workspaces', (route) => route.fulfill({ json: [{
    gateway: 'cloud', host: 'remote.example.test', state: 'reachable',
    workspaces: [{ id: 'notes', name: 'Remote notes', health: 'healthy' }],
  }] }));
  await page.route('**/notes@cloud/**', (route) => route.fulfill({
    contentType: 'text/html', body: '<main>Remote workspace stub</main>',
  }));
  await navigateToApp(page);
  await page.locator('.desktop-header [data-role="brand-menu-toggle"]').click();
  const disclosure = page.locator('.brand-menu [aria-expanded]').filter({ hasText: 'Workspaces' });
  if (await disclosure.getAttribute('aria-expanded') === 'false') await disclosure.click();
  await expect(page.getByText('Manage workspaces', { exact: true })).toBeVisible();
  await expect(page.locator('.brand-menu-ws-row', { hasText: 'Local workspace' })).toBeVisible();
  const remote = page.getByRole('menuitem', { name: /Remote notes/ });
  await expect(remote).toHaveAttribute('href', '/notes@cloud/');
  await remote.click();
  await expect(page).toHaveURL(/\/notes@cloud\//);
  await expect(page.getByText('Remote workspace stub')).toBeVisible();
});
