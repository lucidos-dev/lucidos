// @vitest-environment jsdom
/** The summary tree browser.
 *
 *  It opens the top of a tree, then each line one zoom level at a time, down
 *  to the exact message. A failed read says so instead of reading as empty.
 *  Threads opens any thread's own tree. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type {
  RecallDateResponse,
  RecallZoomResponse,
  SummaryTreeThreadsResponse,
  SummaryTreeTop,
} from '../../../api/types';

const api = vi.hoisted(() => ({
  getSummaryTree: vi.fn<(thread?: string) => Promise<SummaryTreeTop>>(),
  getSummaryTreeThreads: vi.fn<(page: { limit: number; offset: number }) => Promise<SummaryTreeThreadsResponse>>(),
  getRecallZoom: vi.fn<(id: string) => Promise<RecallZoomResponse>>(),
  getRecallDate: vi.fn<(id: string) => Promise<RecallDateResponse>>(),
}));

vi.mock('../../shared/Disclosure', () => import('../../shared/__tests__/disclosureStub'));
vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  ...api,
}));

import { SummaryTreeBrowser, THREADS_PAGE_SIZE } from '../SummaryTreeBrowser';

const THREAD = '3f2c1d4e-0000-4000-8000-000000000000';
const DATE = { from: '2026-09-02T10:00:00Z', to: '2026-09-19T10:00:00Z' };

let host: HTMLDivElement;

/** Reads settle a few microtasks after the render that started them. */
async function settle() {
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
}

async function mount() {
  await act(async () => { render(<SummaryTreeBrowser />, host); });
  await settle();
}

async function click(el: Element | null | undefined) {
  if (!el) throw new Error('nothing to click');
  await act(async () => { (el as HTMLElement).click(); });
  await settle();
}

function line(id: string): HTMLButtonElement {
  const found = [...host.querySelectorAll<HTMLButtonElement>('button.tree-line')]
    .find((b) => b.textContent?.includes(`text of ${id}`));
  if (!found) throw new Error(`no line ${id}`);
  return found;
}

beforeEach(() => {
  for (const fn of Object.values(api)) fn.mockReset();
  api.getRecallDate.mockImplementation(async (id) => ({ id, ...DATE }));
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

describe('the workspace tree', () => {
  beforeEach(() => {
    api.getSummaryTree.mockResolvedValue({
      entries: 9,
      lines: [
        { id: 'w/0+8', text: 'text of w/0+8' },
        { id: 'w/8+1', text: 'text of w/8+1' },
        { id: 'w/pending', text: '1 newer entries are not summarised yet' },
      ],
    });
  });

  it('shows the top lines with their spans and the pending note', async () => {
    await mount();
    expect(api.getSummaryTree).toHaveBeenCalledWith(undefined);
    expect(host.querySelector('.tree-browser-count')?.textContent).toBe('9 entries summarised');
    expect(line('w/0+8').textContent).toContain('8 entries');
    expect(line('w/8+1').textContent).toContain('1 entry');
    expect(host.querySelector('.tree-line-note')?.textContent).toContain('not summarised yet');
    expect(line('w/0+8').getAttribute('aria-expanded')).toBe('false');
  });

  it('opens a line into finer lines, then a leaf into its source', async () => {
    api.getRecallZoom.mockImplementation(async (id) => (id === 'w/0+8'
      ? { id, lines: [{ id: 'w/0+4', text: 'text of w/0+4' }, { id: 'w/4+4', text: 'text of w/4+4' }] }
      : { id, lines: [{ id, text: 'user: the import drops rows' }] }));
    await mount();

    await click(line('w/0+8'));
    expect(api.getRecallZoom).toHaveBeenCalledWith('w/0+8');
    expect(line('w/0+8').getAttribute('aria-expanded')).toBe('true');
    expect(line('w/0+8').querySelector('.tree-line-date')?.textContent).toContain('–');
    const children = host.querySelector('.tree-line-children')!;
    expect(children.textContent).toContain('text of w/0+4');
    expect(children.textContent).toContain('text of w/4+4');

    await click(line('w/8+1'));
    expect(host.querySelector('.tree-line-source')?.textContent).toBe('user: the import drops rows');
  });

  it('still opens a line whose date cannot be read, and says why it has none', async () => {
    api.getRecallZoom.mockResolvedValue({ id: 'w/8+1', lines: [{ id: 'w/8+1', text: '(a thread no longer here)' }] });
    api.getRecallDate.mockRejectedValue(new Error('w/8+1 covers no entry yet'));
    await mount();
    await click(line('w/8+1'));
    expect(host.querySelector('.tree-line-source')?.textContent).toBe('(a thread no longer here)');
    const date = line('w/8+1').querySelector('.tree-line-date');
    expect(date?.textContent).toBe('No date');
    expect(date?.getAttribute('data-tooltip')).toContain('covers no entry yet');
  });

  it('says a line failed to open rather than showing nothing', async () => {
    api.getRecallZoom.mockRejectedValue(new Error('the engine is down'));
    await mount();
    await click(line('w/0+8'));
    expect(host.querySelector('.error-text')?.textContent).toContain('the engine is down');
  });

  it('closes an open line without reading it again', async () => {
    api.getRecallZoom.mockResolvedValue({
      id: 'w/0+8',
      lines: [{ id: 'w/0+4', text: 'text of w/0+4' }, { id: 'w/4+4', text: 'text of w/4+4' }],
    });
    await mount();
    await click(line('w/0+8'));
    await click(line('w/0+8'));
    expect(line('w/0+8').getAttribute('aria-expanded')).toBe('false');
    await click(line('w/0+8'));
    expect(api.getRecallZoom).toHaveBeenCalledTimes(1);
  });
});

it('says when nothing is summarised yet', async () => {
  api.getSummaryTree.mockResolvedValue({ entries: 0, lines: [] });
  await mount();
  expect(host.querySelector('.empty-state')?.textContent).toBe('Nothing is summarised yet');
});

it('shows a failed top as an error, not an empty tree', async () => {
  api.getSummaryTree.mockRejectedValue(new Error('no database'));
  await mount();
  expect(host.querySelector('.error-text')?.textContent).toContain('no database');
  expect(host.querySelector('.empty-state:not(.error-text)')).toBeNull();
});

it('lists the threads with trees and opens one', async () => {
  api.getSummaryTree.mockImplementation(async (thread) => (thread
    ? { entries: 2, lines: [{ id: `${thread}/0+2`, text: `text of ${thread}/0+2` }] }
    : { entries: 0, lines: [] }));
  api.getSummaryTreeThreads.mockResolvedValue({
    threads: [{ thread_id: THREAD, title: 'Weekly report', last_activity: '2026-10-01T10:00:00Z', summarised: 2 }],
    total: 1,
    has_more: false,
  });
  await mount();

  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Threads'));
  expect(host.querySelector('.tree-browser-count')?.textContent).toBe('1 thread');
  const row = host.querySelector('.tree-thread-row')!;
  expect(row.textContent).toContain('Weekly report');
  expect(row.textContent).toContain('2 entries summarised');

  await click(row);
  expect(api.getSummaryTree).toHaveBeenLastCalledWith(THREAD);
  expect(host.querySelector('.tree-browser-thread-title')?.textContent).toBe('Weekly report');
  expect(line(`${THREAD}/0+2`)).toBeTruthy();

  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Back'));
  expect(host.querySelector('.tree-thread-row')).not.toBeNull();
});

it('keeps the thread page across opening a thread and going back', async () => {
  api.getSummaryTree.mockImplementation(async (thread) => (thread
    ? { entries: 1, lines: [{ id: `${thread}/0+1`, text: `text of ${thread}/0+1` }] }
    : { entries: 0, lines: [] }));
  api.getSummaryTreeThreads.mockImplementation(async ({ offset }) => ({
    threads: [{ thread_id: THREAD, title: `Thread at ${offset}`, last_activity: '2026-10-01T10:00:00Z', summarised: 1 }],
    total: 120,
    has_more: offset + THREADS_PAGE_SIZE < 120,
  }));
  await mount();
  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Threads'));
  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Next'));
  expect(host.querySelector('.tree-thread-row')?.textContent).toContain(`Thread at ${THREADS_PAGE_SIZE}`);

  await click(host.querySelector('.tree-thread-row'));
  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Back'));
  expect(host.querySelector('.tree-thread-row')?.textContent).toContain(`Thread at ${THREADS_PAGE_SIZE}`);
  expect(api.getSummaryTreeThreads).toHaveBeenLastCalledWith({ limit: THREADS_PAGE_SIZE, offset: THREADS_PAGE_SIZE });
});

it('steps back a page when a delete empties the one shown', async () => {
  api.getSummaryTree.mockResolvedValue({ entries: 0, lines: [] });
  let total = THREADS_PAGE_SIZE + 10;
  api.getSummaryTreeThreads.mockImplementation(async ({ offset }) => ({
    threads: offset < total
      ? [{ thread_id: THREAD, title: `Thread at ${offset}`, last_activity: '2026-10-01T10:00:00Z', summarised: 1 }]
      : [],
    total,
    has_more: offset + THREADS_PAGE_SIZE < total,
  }));
  await mount();
  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Threads'));
  await click([...host.querySelectorAll('button')].find((b) => b.textContent === 'Next'));
  total = THREADS_PAGE_SIZE - 10;
  const { summaryTreesVersion } = await import('../../../store/store');
  await act(async () => { summaryTreesVersion.value++; });
  await settle();
  await settle();
  expect(host.querySelector('.tree-thread-row')?.textContent).toContain('Thread at 0');
  expect(host.textContent).not.toContain('No threads have a summary tree yet');
});
