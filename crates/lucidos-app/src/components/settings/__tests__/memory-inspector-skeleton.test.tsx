// @vitest-environment jsdom
/** The memory inspector draws its stats strip and entry rows as skeletons
 *  under one delay gate. A memory's source shimmers in its own code block
 *  while it loads, never a "Loading..." button label. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render, type FunctionComponent } from 'preact';
import { act } from 'preact/test-utils';
import type { MemoryEntriesResponse, MemoryStatsResponse } from '../../../api/types';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

const reads = vi.hoisted(() => ({
  entries: (_: MemoryEntriesResponse) => {},
  stats: (_: MemoryStatsResponse) => {},
}));

vi.mock('../../shared/Disclosure', () => import('../../shared/__tests__/disclosureStub'));
vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getMemoryStats: () => new Promise((r) => { reads.stats = r; }),
  getMemoryEntries: () => new Promise((r) => { reads.entries = r; }),
  getMemorySource: () => new Promise(() => {}),
}));

let host: HTMLDivElement;
let MemoryInspector: FunctionComponent;

beforeEach(async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  vi.resetModules();
  ({ MemoryInspector } = await import('../MemoryInspector'));
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<MemoryInspector />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.useRealTimers();
});

it('draws no skeleton before the gate opens', () => {
  expect(host.querySelector('.sk-bar')).toBeNull();
});

it('draws the stats strip and entry rows together once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelectorAll('.loading-fade-skeleton .memory-stats-bar .memory-stat-value .sk-bar')).toHaveLength(3);
  expect(host.querySelector('.loading-fade-skeleton .memory-stats-bar .memory-stat-label')?.textContent).toBe('Total');
  expect(host.querySelectorAll('.loading-fade-skeleton .memory-entry-row').length).toBeGreaterThan(0);
  expect(host.querySelector('.loading-spinner')).toBeNull();
});

it('shimmers a source block while a memory source loads', async () => {
  await act(async () => {
    reads.entries({
      entries: [{
        id: 'm1',
        source: { type: 'event', id: 'e1' },
        topic: 'prefs',
        summary: 'Likes dark mode',
        importance: 0.5,
        entities: [],
        src_created_at: '2026-01-01T00:00:00Z',
        created_at: '2026-01-01T00:00:00Z',
      }],
      total: 1,
      has_more: false,
    } as MemoryEntriesResponse);
  });
  act(() => { host.querySelector<HTMLElement>('.memory-entry-row')!.click(); });
  const button = host.querySelector<HTMLButtonElement>('.memory-view-source-btn')!;
  act(() => { button.click(); });
  expect(button.textContent).toBe('Hide source');
  expect(host.querySelector('.memory-source-pre')).toBeNull();
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(host.querySelector('.loading-fade-skeleton .memory-source-pre .sk-bar')).not.toBeNull();
  expect(host.textContent).not.toContain('Loading');
});
