// @vitest-environment jsdom
/** A thread recent survives a palette opened before the thread list loads.
 *  `threadMap` holds only the loaded window, and it is empty on a cold start,
 *  so its membership never proves a thread is gone. Only the engine does: a
 *  delete, or a tap the engine answers with "not found". */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { SearchResultItem } from '../../../api/client';

const engine = vi.hoisted(() => ({ hasThread: true }));

vi.mock('../../../store/actions/thread-loading', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/thread-loading')>()),
  ensureThreadByIdInMap: async () => engine.hasThread,
}));

import { SearchEverywhere } from '../SearchEverywhere';
import { searchEverywhereOpen, threadMap, threadsLoaded } from '../../../store/store';
import { RECENTS_KEY } from '../../../store/actions/entityReferences';
import { dropDeletedThreads } from '../../../store/actions/threads-delete';

const THREAD_RECENT: SearchResultItem = {
  id: 'thread-outside-the-window',
  title: 'Quarterly report',
  subtitle: '',
  category: 'threads',
  score: 1,
};

let host: HTMLDivElement;

function openPalette() {
  act(() => { searchEverywhereOpen.value = true; });
}

function savedRecents(): SearchResultItem[] {
  return JSON.parse(localStorage.getItem(RECENTS_KEY) ?? '[]');
}

function shownTitles(): string[] {
  return [...document.querySelectorAll('[data-role="search-result"] .search-everywhere-result-title')]
    .map(el => el.textContent ?? '');
}

beforeEach(() => {
  localStorage.setItem(RECENTS_KEY, JSON.stringify([THREAD_RECENT]));
  threadMap.value = new Map();
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<SearchEverywhere />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  searchEverywhereOpen.value = false;
  threadsLoaded.value = false;
  engine.hasThread = true;
  localStorage.removeItem(RECENTS_KEY);
});

describe('thread recents', () => {
  it('keeps a thread recent when the palette opens before the thread list loads', () => {
    threadsLoaded.value = false;
    openPalette();
    expect(shownTitles()).toEqual([THREAD_RECENT.title]);
    expect(savedRecents()).toEqual([THREAD_RECENT]);
  });

  it('keeps a thread recent that is outside the loaded window', () => {
    threadsLoaded.value = true;
    openPalette();
    expect(shownTitles()).toEqual([THREAD_RECENT.title]);
    expect(savedRecents()).toEqual([THREAD_RECENT]);
  });

  it('drops the recent of a thread that was deleted', () => {
    dropDeletedThreads([THREAD_RECENT.id]);
    openPalette();
    expect(shownTitles()).toEqual([]);
    expect(savedRecents()).toEqual([]);
  });

  it('drops the recent when a tap finds the engine no longer has the thread', async () => {
    engine.hasThread = false;
    openPalette();
    const row = document.querySelector<HTMLElement>('[data-role="search-result"]')!;
    await act(async () => {
      row.click();
      await vi.waitFor(() => expect(savedRecents()).toEqual([]));
    });
  });
});
