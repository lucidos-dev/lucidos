// @vitest-environment jsdom
/**
 * Every collapsible drawer header leads with the chevron, and its turn follows
 * the header's `aria-expanded` (styles/section-header.css turns it on that
 * attribute). jsdom loads no CSS, so this pins the DOM the CSS keys on.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadDrawer, setSectionCollapsed } from '../ThreadDrawer';
import { threadMap, threadsLoaded } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';

vi.mock('../../../store/actions/thread-loading', () => ({
  loadThreadEvents: vi.fn(),
  loadOlderThreads: vi.fn(),
  reloadAfterFilterChange: vi.fn(),
  filterChangedSinceLoad: () => false,
  ensureThreadInMap: vi.fn(),
}));

let host: HTMLDivElement;
const header = (label: string) => Array.from(host.querySelectorAll<HTMLElement>('.list-section-title-collapsible'))
  .find(h => h.querySelector('.section-label')?.textContent === label)!;

beforeEach(() => {
  vi.stubGlobal('IntersectionObserver', class { observe() {} disconnect() {} });
  localStorage.clear();
  setSectionCollapsed('current', false);
  threadMap.value = new Map([
    ['a', makeThreadState('a', { meta: { title: 'Current one', section: 'inbox' } })],
    ['p', makeThreadState('p', { meta: { title: 'Pinned one', section: 'inbox', saved: true } })],
  ]);
  threadsLoaded.value = true;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<ThreadDrawer forceVisible />, host); });
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  vi.unstubAllGlobals();
});

describe('a drawer section header', () => {
  it('draws the chevron first, before the section icon', () => {
    for (const label of ['Pinned', 'Current']) {
      const h = header(label);
      expect(h.firstElementChild?.classList.contains('section-chevron'), label).toBe(true);
      expect(h.firstElementChild?.nextElementSibling?.classList.contains('section-icon'), label).toBe(true);
    }
  });

  it('turns the chevron with a collapse, through aria-expanded', () => {
    const h = () => header('Current');
    expect(h().getAttribute('aria-expanded')).toBe('true');
    act(() => { h().click(); });
    expect(h().getAttribute('aria-expanded')).toBe('false');
    expect(h().querySelector(':scope > .section-chevron')).not.toBeNull();
    act(() => { h().click(); });
    expect(h().getAttribute('aria-expanded')).toBe('true');
  });
});
