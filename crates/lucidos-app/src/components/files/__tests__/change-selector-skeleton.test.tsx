// @vitest-environment jsdom
/** The change selector's loading states.
 *
 *  It wears its own trigger box while the list loads. It unmounts when there
 *  is nothing to choose. It appends placeholder rows while a page loads. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ChangeSelector } from '../ChangeSelector';
import { repoChanges, repoChangesLoadingMore } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import type { Change } from '../../../api/client';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  repoChanges.value = { status: 'not-loaded' };
  repoChangesLoadingMore.value = false;
  vi.useRealTimers();
});

function show() {
  act(() => { render(<ChangeSelector />, host); });
}

it('draws the trigger box past the delay gate while the list loads', () => {
  repoChanges.value = { status: 'loading' };
  show();
  expect(host.querySelector('.sk-bar')).toBeNull();
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelector('.loading-fade-skeleton .dropdown-skeleton .sk-bar')).not.toBeNull();
});

it('leaves nothing behind when there are no changes', () => {
  repoChanges.value = { status: 'loaded', data: { pending: [], applied: [], has_more: false } } as never;
  show();
  expect(host.innerHTML).toBe('');
});

it('appends placeholder rows while the next page loads', () => {
  const change = { id: 'c1', description: 'Fix the thing', file_count: 2 } as Change;
  repoChanges.value = { status: 'loaded', data: { pending: [change], applied: [], has_more: true } } as never;
  repoChangesLoadingMore.value = true;
  show();
  act(() => { host.querySelector<HTMLButtonElement>('.dropdown-trigger')!.click(); });
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  const menu = document.querySelector('.change-selector-menu')!;
  expect(menu.querySelectorAll('.loading-fade-skeleton .dropdown-option')).toHaveLength(2);
  expect(menu.textContent).not.toContain('Loading');
});
