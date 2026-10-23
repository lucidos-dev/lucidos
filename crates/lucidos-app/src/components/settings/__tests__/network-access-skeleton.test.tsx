// @vitest-environment jsdom
/** Settings › Network access draws the inherited-bind row as its loading
 *  placeholder, under a title that is there from the first frame. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { NetworkConfigResponse } from '../../../api/types';

const read = vi.hoisted(() => ({ resolve: (_: NetworkConfigResponse) => {} }));

vi.mock('../../../api/client/settings', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/settings')>()),
  getNetworkConfig: () => new Promise<NetworkConfigResponse>((r) => { read.resolve = r; }),
}));

import { NetworkAccessPage } from '../NetworkAccessPage';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<NetworkAccessPage />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('keeps the search anchor on screen and draws no body before the gate opens', () => {
  expect(host.querySelector('[data-search-anchor="access:network"]')).not.toBeNull();
  expect(host.querySelector('.sk-bar')).toBeNull();
});

it('draws the inherited-bind row as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelector('.loading-fade-skeleton .list-row .title .sk-bar')).not.toBeNull();
  expect(host.textContent).not.toContain('Loading');
});

it('shows the stored bind when the read lands', async () => {
  await act(async () => {
    read.resolve({
      inherit: true,
      gateway_bind: '127.0.0.1',
      engine_bind: '127.0.0.1',
      detected_tailscale_ip: null,
    } as NetworkConfigResponse);
  });
  expect(host.querySelector('.loading-fade-content .list-row-details strong')?.textContent).toBe('127.0.0.1');
});
