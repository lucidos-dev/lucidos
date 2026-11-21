// @vitest-environment jsdom
/** File search keeps its field live while the file list loads, and draws
 *  result rows as the placeholder past the delay gate. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FileSearchModal } from '../FileSearchModal';
import { artifacts, changes, fileSearchOpen, repoFiles, repoSource } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  repoSource.value = null;
  artifacts.value = { status: 'loading' };
  fileSearchOpen.value = true;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<FileSearchModal />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  fileSearchOpen.value = false;
  artifacts.value = { status: 'not-loaded' };
  repoFiles.value = { status: 'not-loaded' };
  changes.value = { status: 'not-loaded' };
  repoSource.value = null;
  vi.useRealTimers();
});

function results(): Element {
  return document.querySelector('.file-search-results')!;
}

it('draws the live search field at once and no rows before the gate', () => {
  expect(document.querySelector('[data-role="file-search-input"]')).not.toBeNull();
  expect(results().querySelector('.sk-bar')).toBeNull();
  expect(document.querySelector('.file-search-modal')?.textContent).not.toContain('Loading');
});

it('draws result rows as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(results().querySelectorAll('.loading-fade-skeleton .file-search-result').length).toBeGreaterThan(0);
  expect(results().querySelector('.loading-fade-skeleton .file-search-result-name .sk-bar')).not.toBeNull();
});

it('lists matches when the files land', () => {
  act(() => { artifacts.value = { status: 'loaded', data: ['notes/plan.md'] }; });
  expect(results().querySelector('.loading-fade-content .file-search-result-name')?.textContent).toBe('plan.md');
});

it('keeps change files searchable when the primary source fails', () => {
  act(() => {
    repoSource.value = 'r1';
    repoFiles.value = { status: 'failed', error: 'clone missing' };
    changes.value = { status: 'loaded', data: [{ id: 'c1', files: ['src/app.ts'] }] as never };
  });
  expect(results().textContent).not.toContain('Failed to load files');
  expect(results().querySelector('.file-search-result-name')?.textContent).toBe('app.ts');
});
