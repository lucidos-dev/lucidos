// @vitest-environment jsdom
/** Settings › Access › Other addresses opens on a failed read, because Pair a
 *  device points the user at the error it holds. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getNetworkConfig: () => Promise.reject(new Error('network config unreachable')),
  getTailnetStatus: () => new Promise(() => {}),
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
