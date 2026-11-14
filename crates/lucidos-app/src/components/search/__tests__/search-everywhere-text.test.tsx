// @vitest-environment jsdom
/** Text search in Search Everywhere (ADR 0383). A line shows with its match
 *  marked, after every name section on the All tab. The Text tab lists every
 *  line, and Enter opens the file at that line. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { SearchResultItem, TextSearchHit, TextSearchMode, TextSearchResponse } from '../../../api/client';

const engine = vi.hoisted(() => ({
  files: [] as SearchResultItem[],
  text: {} as Partial<Record<TextSearchMode, TextSearchResponse | 'fail'>>,
  textAsked: [] as { query: string; mode: TextSearchMode }[],
}));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  searchEverywhere: async (_query: string, category: string) => ({
    results: { [category]: category === 'files' ? engine.files : [] },
  }),
  searchText: async (query: string, mode: TextSearchMode) => {
    engine.textAsked.push({ query, mode });
    const answer = engine.text[mode];
    if (answer === undefined) return new Promise(() => {});
    if (answer === 'fail') throw new Error('gateway down');
    return answer;
  },
}));

const navigation = vi.hoisted(() => ({ requests: [] as unknown[][] }));

vi.mock('../../../store/actions/navigation-request', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/navigation-request')>()),
  handleNavigationRequest: (...args: unknown[]) => { navigation.requests.push(args); },
}));

const paneFocus = vi.hoisted(() => ({ focused: [] as unknown[] }));

vi.mock('../../layout/paneFocus', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../layout/paneFocus')>()),
  focusPaneMainControl: (pane: unknown) => { paneFocus.focused.push(pane); },
}));

import { SERVER_DEBOUNCE_MS, SearchEverywhere, TEXT_TAB_WINDOW } from '../SearchEverywhere';
import { preferences, searchEverywhereOpen } from '../../../store/store';
import { RECENTS_KEY } from '../../../store/actions/entityReferences';

/** Matches nothing in the local Settings and Menu indexes. */
const QUERY = 'zqxv needle';

let host: HTMLDivElement;

function line(path: string, lineNo: number): TextSearchHit {
  return { path, line: lineNo, before: 'see the ', matched: 'zqxv NEEDLE', after: ' here' };
}

function found(hits: TextSearchHit[], extra: Partial<{ truncated: boolean; skipped_large_files: number }> = {}): TextSearchResponse {
  return { status: 'ok', hits, truncated: false, skipped_large_files: 0, ...extra };
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

async function settle() {
  await act(async () => { vi.advanceTimersByTime(SERVER_DEBOUNCE_MS); });
  await act(async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); });
}

function results(): Element {
  return document.querySelector('.search-everywhere-results')!;
}

function rows(): HTMLElement[] {
  return [...results().querySelectorAll<HTMLElement>('[data-role="search-result"]')];
}

function headers(): string[] {
  return [...results().querySelectorAll('.search-everywhere-section-header')].map(h => h.textContent ?? '');
}

function textTab(): HTMLButtonElement {
  return [...document.querySelectorAll<HTMLButtonElement>('.search-everywhere-tab')]
    .find(el => el.textContent === 'Text')!;
}

function openTextTab() {
  act(() => { textTab().click(); });
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  engine.files = [];
  engine.text = {};
  engine.textAsked = [];
  navigation.requests = [];
  paneFocus.focused = [];
  localStorage.removeItem(RECENTS_KEY);
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

describe('the All tab', () => {
  it('asks for a preview and lists Text after the name sections', async () => {
    engine.files = [{ id: 'artifacts/zqxv.md', title: 'zqxv.md', subtitle: 'artifacts/zqxv.md', category: 'files', score: 1 }];
    engine.text.preview = found([line('knowhow/notes.md', 4)]);
    type(QUERY);
    await settle();
    expect(engine.textAsked).toEqual([{ query: QUERY, mode: 'preview' }]);
    expect(headers()).toEqual(['files', 'text']);
  });

  it('draws the snippet with the match marked and the line as the subtitle', async () => {
    engine.text.preview = found([line('knowhow/notes.md', 4)]);
    type(QUERY);
    await settle();
    const row = rows()[0];
    expect(row.querySelector('.search-everywhere-result-title')!.textContent).toBe('see the zqxv NEEDLE here');
    expect(row.querySelector('mark.search-match')!.textContent).toBe('zqxv NEEDLE');
    expect(row.querySelector('.search-everywhere-result-subtitle')!.textContent).toBe('knowhow/notes.md:4');
  });

  it('opens the file at the line on Enter, finding the query there, and remembers the file', async () => {
    engine.text.preview = found([line('knowhow/notes.md', 4)]);
    type(QUERY);
    await settle();
    press('Enter');
    expect(navigation.requests).toEqual([
      [{ target: 'file', file_path: 'knowhow/notes.md', line: 4 }, { find: QUERY }],
    ]);
    expect(searchEverywhereOpen.value).toBe(false);
    const recents = JSON.parse(localStorage.getItem(RECENTS_KEY) ?? '[]');
    expect(recents[0]).toMatchObject({ category: 'files', id: 'knowhow/notes.md', title: 'notes.md' });
    expect(paneFocus.focused, 'the find bar takes the focus, not the pane').toEqual([]);
  });

  it('names a failed Text search instead of hiding it', async () => {
    engine.text.preview = 'fail';
    type(QUERY);
    await settle();
    expect(results().textContent).toContain('Text search failed: gateway down');
  });

  it('leaves the Text tab undimmed for a query too short to search', async () => {
    engine.text.preview = { status: 'query-too-short', min_query_chars: 3 };
    type('zq');
    await settle();
    expect(textTab().hasAttribute('data-empty')).toBe(false);
  });

  it('dims the Text tab when no line matched', async () => {
    engine.text.preview = found([]);
    type(QUERY);
    await settle();
    expect(textTab().hasAttribute('data-empty')).toBe(true);
  });

  it('says large files were skipped only when it found no line', async () => {
    engine.text.preview = found([line('a.md', 1)], { skipped_large_files: 2 });
    type(QUERY);
    await settle();
    expect(results().textContent).not.toContain('large files');

    engine.text.preview = found([], { skipped_large_files: 2 });
    type(`${QUERY} again`);
    await settle();
    expect(results().textContent).toContain('Text search skipped 2 large files.');
  });
});

describe('the Text tab', () => {
  it('asks for every line and states the totals', async () => {
    engine.text.all = found([line('a.md', 1), line('a.md', 9), line('b.md', 2)]);
    openTextTab();
    type(QUERY);
    await settle();
    expect(engine.textAsked).toContainEqual({ query: QUERY, mode: 'all' });
    expect(results().querySelector('.search-everywhere-summary')!.textContent).toBe('3 matches in 2 files');
    expect(rows()).toHaveLength(3);
  });

  it('draws a window of lines and grows it as the selection moves past its end', async () => {
    const many = Array.from({ length: TEXT_TAB_WINDOW * 2 + 50 }, (_, i) => line('big.md', i + 1));
    engine.text.all = found(many);
    openTextTab();
    type(QUERY);
    await settle();
    expect(rows()).toHaveLength(TEXT_TAB_WINDOW);
    for (let i = 0; i <= TEXT_TAB_WINDOW; i++) press('ArrowDown');
    expect(rows()).toHaveLength(TEXT_TAB_WINDOW * 2);
    expect(rows()[TEXT_TAB_WINDOW].classList.contains('selected')).toBe(true);
  });

  it('grows the window on scroll without pulling the list back to the selection', async () => {
    const scrolled = vi.spyOn(Element.prototype, 'scrollIntoView').mockImplementation(() => {});
    engine.text.all = found(Array.from({ length: TEXT_TAB_WINDOW * 2 }, (_, i) => line('big.md', i + 1)));
    openTextTab();
    type(QUERY);
    await settle();
    press('ArrowDown');
    expect(scrolled).toHaveBeenCalledTimes(1);

    // Scrolled to within a screen of the end: scrollHeight - scrollTop - clientHeight < clientHeight.
    const list = results() as HTMLElement;
    Object.defineProperty(list, 'scrollHeight', { configurable: true, value: 1000 });
    Object.defineProperty(list, 'clientHeight', { configurable: true, value: 300 });
    list.scrollTop = 600;
    act(() => { list.dispatchEvent(new Event('scroll')); });
    expect(rows()).toHaveLength(TEXT_TAB_WINDOW * 2);
    expect(scrolled).toHaveBeenCalledTimes(1);
    scrolled.mockRestore();
  });

  it('asks the engine how much to type, and says so', async () => {
    engine.text.all = { status: 'query-too-short', min_query_chars: 3 };
    openTextTab();
    type('ab');
    await settle();
    expect(results().textContent).toContain('Type at least 3 characters to search inside files');
  });

  it('says when it stopped at the cap', async () => {
    engine.text.all = found([line('a.md', 1), line('a.md', 2)], { truncated: true });
    openTextTab();
    type(QUERY);
    await settle();
    expect(results().textContent).toContain('Showing the first 2 matches. Refine your query to see the rest.');
  });
});
