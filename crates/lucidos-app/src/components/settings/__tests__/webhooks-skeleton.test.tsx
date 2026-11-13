// @vitest-environment jsdom
/** Settings › Webhooks draws webhook rows as its loading placeholder, past the
 *  delay gate, never a line of "Loading…" text. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { Webhook } from '../../../api/client';

const list = vi.hoisted(() => ({ resolve: (_: Webhook[]) => {} }));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  fetchWebhooks: () => new Promise<Webhook[]>((r) => { list.resolve = r; }),
}));
vi.mock('../../../store/actions/credentials', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/credentials')>()),
  loadCredentials: () => Promise.resolve(),
}));

import { WebhooksPage } from '../WebhooksPage';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<WebhooksPage />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('draws nothing in the list before the delay gate opens', () => {
  expect(host.querySelector('.sk-bar')).toBeNull();
  expect(host.querySelector('.list-row')).toBeNull();
});

it('draws webhook rows as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelectorAll('.loading-fade-skeleton .list-row').length).toBeGreaterThan(0);
  expect(host.querySelector('.loading-fade-skeleton .list-row .title .sk-bar')).not.toBeNull();
  expect(host.textContent).not.toContain('Loading');
});

it('shows the loaded rows when the list lands', async () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  await act(async () => { list.resolve([]); });
  expect(host.querySelector('.loading-fade-content .empty-state')?.textContent).toBe('No webhooks yet.');
});
