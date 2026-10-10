// @vitest-environment jsdom
/** While the next page of notifications loads, the list appends two
 *  notification rows as placeholders past the delay gate. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getNotifications: () => new Promise(() => {}),
}));

import { NotificationsView } from '../NotificationsView';
import { notifications, notificationsFilter, notificationsHasMore, notificationsLoadingMore } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} unobserve() {} });
  notificationsFilter.value = 'all';
  notifications.value = {
    status: 'loaded',
    data: [{ id: 'n1', title: 'Build finished', message: 'All green', read: true, created_at: new Date().toISOString() }],
  };
  notificationsHasMore.value = true;
  notificationsLoadingMore.value = true;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<NotificationsView />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  notifications.value = { status: 'not-loaded' };
  notificationsHasMore.value = false;
  notificationsLoadingMore.value = false;
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it('appends no placeholder before the gate opens', () => {
  expect(host.querySelector('.sk-bar')).toBeNull();
  expect(host.textContent).not.toContain('Loading');
});

it('appends two notification rows once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  const appended = [...host.querySelectorAll('.loading-fade-skeleton')].filter((s) => s.querySelector('.sk-bar'));
  expect(appended).toHaveLength(1);
  expect(appended[0]!.children[0]!.children).toHaveLength(2);
});
