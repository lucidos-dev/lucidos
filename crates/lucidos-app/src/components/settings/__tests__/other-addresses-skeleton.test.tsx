// @vitest-environment jsdom
/** Settings › Access › Other addresses draws URL rows as its loading placeholder,
 *  under a toggle that is there from the first frame. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getNetworkConfig: () => new Promise(() => {}),
  getTailnetStatus: () => new Promise(() => {}),
}));

import { MobileAccessPage } from '../MobileAccessPage';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import { settingsScrollTarget } from '../../../store/store';

let host: HTMLDivElement;

function section(): Element {
  return host.querySelector('[data-search-anchor="access:addresses"]')!.closest('.settings-section')!;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<MobileAccessPage />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('keeps the anchor and draws no rows before the gate opens', () => {
  expect(section().querySelector('.sk-bar')).toBeNull();
});

it('starts folded', () => {
  expect(section().querySelector('[aria-expanded]')?.getAttribute('aria-expanded')).toBe('false');
});

it('opens for a search hit that lands while the page is already shown', () => {
  act(() => { settingsScrollTarget.value = 'access:addresses'; });
  expect(section().querySelector('[aria-expanded]')?.getAttribute('aria-expanded')).toBe('true');
  settingsScrollTarget.value = null;
});

it('draws URL rows as the skeleton once opened and the gate opens', () => {
  act(() => { section().querySelector<HTMLButtonElement>('[aria-expanded]')!.click(); });
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(section().querySelectorAll('.loading-fade-skeleton .list-row')).toHaveLength(2);
  expect(section().querySelector('.loading-fade-skeleton .title .sk-bar')).not.toBeNull();
  expect(section().textContent).not.toContain('Loading');
});
