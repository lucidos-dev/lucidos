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

function tab(label: string): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>('.search-everywhere-tab')]
    .find(el => el.firstChild?.textContent === label)!;
}

function tabCount(label: string): string | null {
  return tab(label).querySelector('.search-everywhere-tab-count')?.textContent ?? null;
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

  it('asks each category for one more than the All tab shows, to tell 5 from 5+', async () => {
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(new Set(engine.limits)).toEqual(new Set([6]));
  });

  it('asks a category tab for its own full page beside the overview', async () => {
    act(() => { tab('Threads').click(); });
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    const page = engine.asked.flatMap((c, i) => (engine.limits[i] === undefined ? [c] : []));
    expect(page).toEqual(['threads']);
    expect(engine.asked.filter((_, i) => engine.limits[i] === 6).sort()).toEqual([...SERVER_CATEGORIES].sort());
  });

  it('does not ask the overview again when the tab changes', async () => {
    answerAll([]);
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    const overviewAsks = engine.limits.filter(l => l === 6).length;
    act(() => { tab('Files').click(); });
    await advance(DEBOUNCE_MS);
    expect(engine.limits.filter(l => l === 6).length).toBe(overviewAsks);
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

    // Neither title holds the query, so the tie keeps Apps above Settings.
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

describe('ranking and tab counts', () => {
  it('lists a section with an exact title above one that lists first by default', async () => {
    answerAll([]);
    engine.answers.apps = [{ ...hit('apps', 'habit-tracker'), title: 'Habit Tracker', subtitle: 'daily settings' }];
    type('settings');
    await advance(DEBOUNCE_MS);
    const headers = [...results().querySelectorAll('.search-everywhere-section-header')].map(h => h.textContent);
    expect(headers.slice(0, 2)).toEqual(['menu', 'settings']);
    expect(headers[headers.length - 1]).toBe('apps');
    expect(rowTitles()[0]).toBe('Settings');
  });

  it('counts each tab once its category answers, capped at 5+', async () => {
    answerAll([]);
    engine.answers.files = Array.from({ length: 6 }, (_, i) => hit('files', `f${i}`));
    engine.answers.apps = [hit('apps', 'habit-tracker')];
    engine.answers.threads = 'hang';
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(tabCount('Files')).toBe('5+');
    expect(tabCount('Apps')).toBe('1');
    expect(tab('Files').getAttribute('aria-label')).toBe('Files, 5+ hits');
    expect(tab('Apps').getAttribute('aria-label')).toBe('Apps, 1 hit');
    expect(tab('Triggers').hasAttribute('data-empty')).toBe(true);
    expect(tab('Triggers').getAttribute('aria-label')).toBe('Triggers, no hits');
    // Still out: no count and not dimmed, so it never reads as empty.
    expect(tabCount('Threads')).toBeNull();
    expect(tab('Threads').hasAttribute('data-empty')).toBe(false);
    expect(tab('Threads').hasAttribute('aria-label')).toBe(false);
  });

  it('never lets a late answer for an old query count the new one', async () => {
    let landOld: (items: SearchResultItem[]) => void = () => {};
    answerAll([]);
    engine.answers.files = new Promise(resolve => { landOld = resolve; });
    type('first');
    await advance(DEBOUNCE_MS);
    engine.answers.files = 'hang';
    type('second');
    await advance(DEBOUNCE_MS);
    await act(async () => {
      landOld(Array.from({ length: 6 }, (_, i) => hit('files', `old${i}`)));
      for (let i = 0; i < 5; i++) await Promise.resolve();
    });
    expect(tabCount('Files')).toBeNull();
    expect(rowTitles().some(t => t.startsWith('old'))).toBe(false);
  });

  it('never dims a tab whose category failed', async () => {
    answerAll([]);
    engine.answers.threads = 'fail';
    type(NO_LOCAL_HITS);
    await advance(DEBOUNCE_MS);
    expect(tab('Threads').hasAttribute('data-empty')).toBe(false);
    expect(tabCount('Threads')).toBeNull();
  });

  it('shows no counts before a query is typed', () => {
    expect(document.querySelector('.search-everywhere-tab-count')).toBeNull();
    expect(document.querySelector('.search-everywhere-tab[data-empty]')).toBeNull();
  });
});
