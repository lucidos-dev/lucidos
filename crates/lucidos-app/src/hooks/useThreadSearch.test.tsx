// @vitest-environment jsdom
/** A thread search closed while its request is in flight never writes results.
 *
 *  The hook aborts from an effect, a frame after the close. Within that frame
 *  the response could land, and a quick reopen made the open flag lie.
 *
 *  Plan: `docs/plans/2026-10-02-shortcut-coverage-and-search-landing.md`. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { threadSearchResults } from '../store/store';
import { openThreadSearch, closeThreadSearch } from '../store/threadSearch';
import { useThreadSearch } from './useThreadSearch';

const pending: Array<(v: unknown[]) => void> = [];
vi.mock('../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api/threads')>()),
  // Never aborts: the guard under test must hold without the abort.
  searchThreads: () => new Promise((resolve) => pending.push(resolve)),
}));

let typeQuery: (q: string) => void = () => {};

function Probe() {
  const { onSearchInput } = useThreadSearch();
  typeQuery = (q) => onSearchInput({ target: { value: q } } as unknown as Event);
  return null;
}

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers();
  pending.length = 0;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => render(<Probe />, host));
  act(() => openThreadSearch());
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
  closeThreadSearch();
  vi.useRealTimers();
});

describe('useThreadSearch', () => {
  it('drops a response for a search closed and reopened before it landed', async () => {
    typeQuery('deploy');
    vi.advanceTimersByTime(300);
    expect(pending).toHaveLength(1);
    // Close and reopen in one task, before the hook's effect can abort.
    closeThreadSearch();
    openThreadSearch();
    pending[0]([{ thread_id: 'stale' }]);
    await Promise.resolve();
    expect(threadSearchResults.value.status).toBe('not-loaded');
  });

  it('writes a response for a search still open', async () => {
    typeQuery('deploy');
    vi.advanceTimersByTime(300);
    pending[0]([]);
    await Promise.resolve();
    expect(threadSearchResults.value.status).toBe('loaded');
  });
});
