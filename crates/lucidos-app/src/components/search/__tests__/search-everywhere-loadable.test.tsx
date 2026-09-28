// @vitest-environment jsdom
/** Search Everywhere holds its results in a Loadable. A slow search draws
 *  result rows past the delay gate, and a failed one says it failed rather
 *  than "No results". */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const search = vi.hoisted(() => ({ fail: false }));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  searchEverywhere: () => (search.fail ? Promise.reject(new Error('gateway down')) : new Promise(() => {})),
}));

import { SearchEverywhere } from '../SearchEverywhere';
import { searchEverywhereOpen } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

function type(text: string) {
  const input = document.querySelector<HTMLInputElement>('.search-everywhere-input')!;
  act(() => {
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

function results(): Element {
  return document.querySelector('.search-everywhere-results')!;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  search.fail = false;
  searchEverywhereOpen.value = true;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<SearchEverywhere />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  searchEverywhereOpen.value = false;
  vi.useRealTimers();
});

it('draws result rows past the delay gate while a search runs', () => {
  type('deploy');
  expect(results().querySelector('.sk-bar')).toBeNull();
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(results().querySelectorAll('.loading-fade-skeleton .search-everywhere-result').length).toBeGreaterThan(0);
  expect(results().querySelector('[data-role="search-result"]')).toBeNull();
});

it('says a failed search failed instead of showing no results', async () => {
  search.fail = true;
  type('deploy');
  await act(async () => { vi.advanceTimersByTime(300); });
  expect(results().textContent).toContain('Search failed: gateway down');
  expect(results().textContent).not.toContain('No results');
});

it('holds the next search behind the delay gate after a cleared one', () => {
  type('dep');
  type('');
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  type('deploy');
  expect(results().querySelector('.sk-bar')).toBeNull();
});
