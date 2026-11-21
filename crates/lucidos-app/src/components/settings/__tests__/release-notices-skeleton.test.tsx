// @vitest-environment jsdom
/** Settings › System › Release notices draws a notice row while the list
 *  loads, past the delay gate. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../store/actions/releaseNotices', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/releaseNotices')>()),
  loadReleaseNotices: () => Promise.resolve(),
}));

import { ReleaseNoticesPage } from '../ReleaseNoticesPage';
import { releaseNoticeView } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  releaseNoticeView.value = { status: 'loading' };
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<ReleaseNoticesPage />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  releaseNoticeView.value = { status: 'not-loaded' };
  vi.useRealTimers();
});

it('keeps its title and draws no skeleton before the gate opens', () => {
  expect(host.querySelector('[data-search-anchor="release-notices:list"]')).not.toBeNull();
  expect(host.querySelector('.sk-bar')).toBeNull();
});

it('draws a notice row as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelector('.loading-fade-skeleton .release-notice-row .release-notice-row-title .sk-bar')).not.toBeNull();
});

it('says there is nothing to do once an empty list lands', () => {
  act(() => { releaseNoticeView.value = { status: 'loaded', data: { notices: [], next_id: null } as never }; });
  expect(host.querySelector('.loading-fade-content .empty-state')?.textContent).toContain('Nothing to do');
});
