// @vitest-environment jsdom
// The Thread queue's Running and Queued sections collapse from their headers,
// and the Capacity policy button sits beside the Running toggle.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../store/actions/threads', () => ({
  focusThreadOrBootstrap: vi.fn(),
}));
vi.mock('../../store/actions/threadQueue', () => ({
  dropQueueEntry: vi.fn(),
  loadThreadQueue: vi.fn(() => Promise.resolve()),
  runQueueEntryNow: vi.fn(),
  saveCapacityPolicy: vi.fn(),
}));

import { ThreadQueueView } from './ThreadQueueView';
import { threadQueue, collapsedThreadQueueSectionIds } from '../../store/store';
import type { CapacityPolicy } from '../../store/types';

const POLICY: CapacityPolicy = {
  max_concurrent_total: 4,
  max_concurrent_event_trigger: 2,
  max_concurrent_cron: 2,
  max_concurrent_sub_thread: 2,
  max_concurrent_coding_agent: 2,
  max_concurrent_per_trigger: 1,
  max_queued_per_trigger: 10,
  reserved_background: 1,
  max_event_trigger_depth: 5,
  max_concurrent_children_per_thread: 10,
  overflow: 'drop-oldest',
};

let host: HTMLDivElement;
const header = (label: string) =>
  Array.from(host.querySelectorAll<HTMLElement>('.list-section-title'))
    .find(h => h.querySelector('.section-label')?.textContent === label)!;

beforeEach(() => {
  localStorage.clear();
  collapsedThreadQueueSectionIds.value = new Set();
  threadQueue.value = {
    status: 'loaded',
    data: {
      entries: [{
        id: 'q1', kind: 'cron', summary: 'Morning digest', status: 'admitted',
        queued_at: '2026-01-01T00:00:00Z', admitted_at: '2026-01-01T00:00:01Z',
      }],
      policy: POLICY,
    },
  };
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('Thread queue sections', () => {
  it('counts Running against the total cap and Queued on its own', async () => {
    await act(() => { render(<ThreadQueueView />, host); });
    expect(header('Running').querySelector('.section-count-open')?.textContent).toBe('1/4');
    expect(header('Queued').querySelector('.section-count-open')?.textContent).toBe('0');
  });

  it('collapses Queued from its header, remembered per device', async () => {
    await act(() => { render(<ThreadQueueView />, host); });
    expect(host.querySelector('[data-role="thread-queue-empty"]')).not.toBeNull();

    await act(() => { header('Queued').querySelector<HTMLButtonElement>('.list-section-toggle')!.click(); });
    expect(header('Queued').classList.contains('collapsed')).toBe(true);
    expect(JSON.parse(localStorage.getItem('lucidos-collapsed-thread-queue-sections')!)).toEqual(['queued']);
  });

  it('keeps the policy button beside the Running toggle', async () => {
    await act(() => { render(<ThreadQueueView />, host); });
    const running = header('Running');
    const button = running.querySelector('.list-section-actions [data-role="toggle-capacity-policy"]');
    expect(button).not.toBeNull();
    expect(running.querySelector('.list-section-toggle [data-role="toggle-capacity-policy"]')).toBeNull();
    expect(button?.getAttribute('aria-expanded')).toBe('false');
  });
});
