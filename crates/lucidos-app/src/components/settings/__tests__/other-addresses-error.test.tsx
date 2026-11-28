// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../utils/basePath', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/basePath')>()),
  WORKSPACE_ID: 'dev',
}));

vi.mock('../../../api/client/control', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/control')>()),
  getGatewayNetworkConfig: () => Promise.reject(new Error('network config unreachable')),
  getGatewayTailnetStatus: () => new Promise(() => {}),
  getExternalAddress: () => new Promise(() => {}),
  getGatewayTls: () => new Promise(() => {}),
}));


import { MobileAccessPage } from '../MobileAccessPage';

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

it('opens the fold and shows the error when an address read fails', async () => {
  await act(async () => { render(<MobileAccessPage />, host); });
  await act(async () => {});
  const toggle = host.querySelector('[data-search-anchor="access:addresses"]')!;
  expect(toggle.getAttribute('aria-expanded')).toBe('true');
  expect(toggle.closest('.settings-section')!.textContent).toContain('network config unreachable');
});
