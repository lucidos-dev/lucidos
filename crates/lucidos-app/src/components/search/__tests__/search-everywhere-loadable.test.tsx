// @vitest-environment jsdom
/** Search Everywhere shows every hit as soon as it has it. Local hits land on
 *  the keystroke and each engine category on its own answer. A slow category
 *  leaves a trailing row saying what is still out. With nothing to show, a
 *  slow search draws result rows past the delay gate, and a failed one says it
 *  failed rather than "No results". */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { SearchResultItem } from '../../../api/client';

type Answer = 'hang' | 'fail' | SearchResultItem[] | Promise<SearchResultItem[]>;

const engine = vi.hoisted(() => ({
  answers: {} as Record<string, Answer>,
  asked: [] as string[],
  limits: [] as (number | undefined)[],
}));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  searchEverywhere: async (_query: string, category: string, opts: { limit?: number } = {}) => {
    engine.asked.push(category);
    engine.limits.push(opts.limit);
    const answer = engine.answers[category] ?? 'hang';
    if (answer === 'hang') return new Promise(() => {});
    if (answer === 'fail') throw new Error('gateway down');
    return { results: { [category]: await answer } };
  },
}));

import { SearchEverywhere } from '../SearchEverywhere';
import { preferences, searchEverywhereOpen } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

/** Matches nothing the frontend holds, so every row has to come from the engine. */
const NO_LOCAL_HITS = 'zqxv';
/** Finds the Search everywhere shortcut in the local settings index. */
const SHORTCUT_QUERY = 'cmd k';
const DEBOUNCE_MS = 150;
const SERVER_CATEGORIES = ['apps', 'files', 'threads', 'triggers', 'changes'];

let host: HTMLDivElement;

function hit(category: string, id: string): SearchResultItem {
  return { id, title: id, subtitle: '', category, score: 1 };
}

function answerAll(answer: Answer) {
  for (const c of SERVER_CATEGORIES) engine.answers[c] = answer;
}

function type(text: string) {
  const input = document.querySelector<HTMLInputElement>('.search-everywhere-input')!;
  act(() => {
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

function press(key: string) {
  const input = document.querySelector<HTMLInputElement>('.search-everywhere-input')!;
  act(() => { input.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true })); });
}

async function advance(ms: number) {
  await act(async () => { vi.advanceTimersByTime(ms); });
}

function results(): Element {
  return document.querySelector('.search-everywhere-results')!;
}

function rowTitles(): string[] {
  return [...results().querySelectorAll('[data-role="search-result"] .search-everywhere-result-title')]
    .map(el => el.textContent ?? '');
}

function selectedTitle(): string | null | undefined {
  return results().querySelector('.selected .search-everywhere-result-title')?.textContent;
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  engine.answers = {};
  engine.asked = [];
  engine.limits = [];
  preferences.value = { status: 'not-loaded' };
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

describe('hits before everything has loaded', () => {
  it('shows local hits on the keystroke, before the engine is asked', () => {
    type(SHORTCUT_QUERY);
    expect(engine.asked).toEqual([]);
    expect(rowTitles().length).toBeGreaterThan(0);
  });

  it('asks the engine once per category, so no category waits on another', async () => {
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect([...engine.asked].sort()).toEqual([...SERVER_CATEGORIES].sort());
  });

  it('asks each category for no more than the All tab shows', async () => {
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(new Set(engine.limits)).toEqual(new Set([5]));
  });

  it('asks a category tab for its own full page', async () => {
    const threadsTab = [...document.querySelectorAll('.search-everywhere-tab')]
      .find(el => el.textContent === 'Threads') as HTMLButtonElement;
    act(() => { threadsTab.click(); });
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(engine.asked).toEqual(['threads']);
    expect(engine.limits).toEqual([undefined]);
  });

  it('drops the engine answers on close, so a reopened palette asks again', async () => {
    engine.answers.apps = [hit('apps', 'habit-tracker')];
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(rowTitles()).toEqual(['habit-tracker']);

    // The shortcut and the header toggle close through the signal, not close().
    act(() => { searchEverywhereOpen.value = false; });
    engine.answers.apps = 'hang';
    act(() => { searchEverywhereOpen.value = true; });
    expect(rowTitles()).toEqual([]);
  });

  it('draws a category that answered while a slower one is still out', async () => {
    engine.answers.apps = [hit('apps', 'habit-tracker')];
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(rowTitles()).toEqual(['habit-tracker']);
  });

  it('ends the list with a row naming what is still being searched', async () => {
    answerAll([]);
    engine.answers.threads = 'hang';
    type(SHORTCUT_QUERY);
    await advance(SPINNER_DELAY_MS);
    const last = results().querySelector('.loading-fade-content')!.lastElementChild!;
    expect(last.classList.contains('search-everywhere-pending')).toBe(true);
    expect(last.textContent).toBe('Searching threads');
    expect(results().querySelector('.sk-bar')).toBeNull();
  });

  it('draws no pending row once every category has answered', async () => {
    answerAll([]);
    type(SHORTCUT_QUERY);
    await advance(SPINNER_DELAY_MS);
    expect(results().querySelector('.search-everywhere-pending')).toBeNull();
  });

  it('keeps the keyboard selection on its row when a category lands above it', async () => {
    let landApps: (items: SearchResultItem[]) => void = () => {};
    answerAll([]);
    engine.answers.apps = new Promise(resolve => { landApps = resolve; });
    type(SHORTCUT_QUERY);
    await advance(DEBOUNCE_MS);
    const first = rowTitles()[0];
    press('ArrowDown');
    expect(selectedTitle()).toBe(first);

    // Apps sort above settings in the All tab.
    await act(async () => {
      landApps([hit('apps', 'habit-tracker')]);
      for (let i = 0; i < 5; i++) await Promise.resolve();
    });
    expect(rowTitles()[0]).toBe('habit-tracker');
    expect(selectedTitle()).toBe(first);
  });
});

describe('loading and failure', () => {
  it('draws result rows past the delay gate while a search with no hits runs', () => {
    type(NO_LOCAL_HITS);
    expect(results().querySelector('.sk-bar')).toBeNull();
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    expect(results().querySelectorAll('.loading-fade-skeleton .search-everywhere-result').length).toBeGreaterThan(0);
    expect(results().querySelector('[data-role="search-result"]')).toBeNull();
  });

  it('says a failed search failed instead of showing no results', async () => {
    answerAll('fail');
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(results().textContent).toContain('Search failed: gateway down');
    expect(results().textContent).not.toContain('No results');
  });

  it('names the one category that failed and keeps the hits that landed', async () => {
    answerAll([]);
    engine.answers.apps = [hit('apps', 'habit-tracker')];
    engine.answers.threads = 'fail';
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(rowTitles()).toEqual(['habit-tracker']);
    expect(results().textContent).toContain('Threads search failed: gateway down');
  });

  it('holds the next search behind the delay gate after a cleared one', () => {
    type('dep');
    type('');
    act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
    type(NO_LOCAL_HITS);
    expect(results().querySelector('.sk-bar')).toBeNull();
  });
});
