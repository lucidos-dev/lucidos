// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('./app-bridge', () => ({ askFrame: vi.fn(), tellFrame: vi.fn() }));
vi.mock('./apps', () => ({ getVisibleAppFrame: vi.fn() }));
// jsdom lays nothing out, so every element would read as hidden.
vi.mock('../../components/chat/scrollState', () => ({ isElementVisible: () => true }));

import { askFrame, tellFrame } from './app-bridge';
import { getVisibleAppFrame } from './apps';
import {
  FIND_TIMEOUT_MS, closeFind, contentFindKind, findFocusRequest, findQuery, findResult, findStatusText,
  findSurface, focusedFindSurface, openFind, parseFindReply, rerunFindOn, resetFind, setFindQuery, stepFind,
  takeFindRequest, toggleFocusedFind,
} from './find-bar';
import { MIN_FIND_QUERY_CHARS, fileFindScope, requestFind } from './find-request';
import {
  filePreviewEditing, filePreviewSource, findRequest, focusedPane, lineScrollTarget, panelOverlay,
} from '../store';

const frame = { contentWindow: {} } as unknown as HTMLIFrameElement;
const APP = { type: 'app-ui', app: { id: 'habit-tracker' } } as unknown as typeof panelOverlay.value;
const file = (path: string) => ({ type: 'file-preview', path }) as typeof panelOverlay.value;

// jsdom has no layout. Every range sits at the top of the viewport.
Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 10, 10);

beforeEach(() => {
  vi.useFakeTimers();
  vi.mocked(askFrame).mockReset();
  vi.mocked(tellFrame).mockReset();
  vi.mocked(getVisibleAppFrame).mockReturnValue(frame);
  panelOverlay.value = APP;
  filePreviewSource.value = false;
  filePreviewEditing.value = false;
  focusedPane.value = 'content';
  document.body.innerHTML = '';
  resetFind();
});

afterEach(() => {
  vi.useRealTimers();
});

/** Type a query and let the typing pause run it. */
async function typeQuery(query: string) {
  setFindQuery(query);
  await vi.runAllTimersAsync();
}

describe('parseFindReply', () => {
  it('takes a well-formed count', () => {
    expect(parseFindReply({ total: 12, current: 3, capped: false })).toEqual({ total: 12, current: 3, capped: false });
  });

  it('takes zero of zero as no matches', () => {
    expect(parseFindReply({ total: 0, current: 0 })).toEqual({ total: 0, current: 0, capped: false });
  });

  it('refuses anything the frame could have mangled', () => {
    for (const bad of [
      null, 'x', {}, { total: -1, current: 0 }, { total: 2, current: 3 }, { total: 1.5, current: 1 },
      { total: '3', current: 1 }, { total: 3, current: 0 }, { total: 0, current: 1 },
    ]) {
      expect(parseFindReply(bad)).toBeNull();
    }
  });
});

describe('what the content pane can search', () => {
  it('searches an app, and a text preview', () => {
    expect(contentFindKind()).toBe('app');
    for (const path of ['artifacts/notes.md', 'artifacts/data.csv', 'artifacts/run.py', 'knowhow/a.txt']) {
      panelOverlay.value = file(path);
      expect(contentFindKind(), path).toBe('page');
    }
    panelOverlay.value = file('artifacts/report.html');
    expect(contentFindKind(), 'the HTML preview is its own isolated frame').toBe('preview-frame');
    filePreviewSource.value = true;
    expect(contentFindKind(), 'its source view is host DOM').toBe('page');
  });

  it('has nothing to search in a picture, a PDF, the editor, or a settings page', () => {
    for (const path of ['artifacts/photo.png', 'artifacts/report.pdf']) {
      panelOverlay.value = file(path);
      expect(contentFindKind(), path).toBeNull();
    }
    panelOverlay.value = file('artifacts/notes.md');
    filePreviewEditing.value = true;
    expect(contentFindKind()).toBeNull();
    panelOverlay.value = null;
    expect(contentFindKind()).toBeNull();
  });

  it('gives Mod+F to the focused pane only when that pane can be searched', () => {
    expect(focusedFindSurface()).toBe('content');
    panelOverlay.value = file('artifacts/photo.png');
    expect(focusedFindSurface()).toBeNull();
    panelOverlay.value = APP;
    focusedPane.value = 'drawer';
    expect(focusedFindSurface()).toBeNull();
  });
});

describe('the find bar over an app', () => {
  it('asks the open app to find the typed text, once the typing pauses', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 4, current: 1, capped: false });
    openFind('content');
    setFindQuery('ap');
    setFindQuery('apple');
    expect(askFrame).not.toHaveBeenCalled();
    await vi.runAllTimersAsync();
    expect(askFrame).toHaveBeenCalledTimes(1);
    expect(askFrame).toHaveBeenCalledWith(frame, 'find', { query: 'apple' }, FIND_TIMEOUT_MS);
    expect(findResult.value).toEqual({ status: 'found', total: 4, current: 1, capped: false });
  });

  it('steps forward and back through the matches', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 4, current: 2, capped: false });
    openFind('content');
    await typeQuery('apple');
    await stepFind(1);
    expect(askFrame).toHaveBeenLastCalledWith(frame, 'find', { query: 'apple', step: 1 }, FIND_TIMEOUT_MS);
    await stepFind(-1);
    expect(askFrame).toHaveBeenLastCalledWith(frame, 'find', { query: 'apple', step: -1 }, FIND_TIMEOUT_MS);
  });

  it('says the app cannot be searched when the frame does not answer, never "No matches"', async () => {
    vi.mocked(askFrame).mockRejectedValue(new Error('the app did not answer "find" in 2000ms'));
    openFind('content');
    await typeQuery('apple');
    expect(findStatusText(findResult.value)).toBe('This app can’t be searched');
  });

  it('says the same for a reply it cannot read', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 'lots' });
    openFind('content');
    await typeQuery('apple');
    expect(findResult.value.status).toBe('unsearchable');
  });

  it('lets only the newest request land', async () => {
    let answerFirst: (v: unknown) => void = () => {};
    vi.mocked(askFrame)
      .mockReturnValueOnce(new Promise((r) => { answerFirst = r; }))
      .mockResolvedValueOnce({ total: 1, current: 1, capped: false });
    openFind('content');
    await typeQuery('ab');
    await typeQuery('abc');
    answerFirst({ total: 9, current: 1, capped: false });
    await vi.runAllTimersAsync();
    expect(findResult.value).toEqual({ status: 'found', total: 1, current: 1, capped: false });
  });

  it('takes the highlights out of the app when it closes', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 1, current: 1, capped: false });
    openFind('content');
    await typeQuery('apple');
    closeFind();
    expect(tellFrame).toHaveBeenCalledWith(frame, 'find', { clear: true });
    expect(findSurface.value).toBeNull();
    expect(findQuery.value).toBe('');
    expect(findResult.value).toEqual({ status: 'idle' });
  });

  it('drops the old count when the app reloads, and searches again', async () => {
    vi.mocked(askFrame).mockResolvedValueOnce({ total: 5, current: 2, capped: false });
    openFind('content');
    await typeQuery('apple');
    let answer: (v: unknown) => void = () => {};
    vi.mocked(askFrame).mockReturnValueOnce(new Promise((r) => { answer = r; }));
    const rerun = rerunFindOn('content');
    expect(findResult.value).toEqual({ status: 'idle' });
    answer({ total: 1, current: 1, capped: false });
    await rerun;
    expect(findResult.value).toEqual({ status: 'found', total: 1, current: 1, capped: false });
  });

  it('waits for a second character, and says so', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 4, current: 1, capped: false });
    openFind('content');
    await typeQuery('a');
    expect(askFrame).not.toHaveBeenCalled();
    expect(findResult.value).toEqual({ status: 'too-short' });
    expect(findStatusText(findResult.value)).toBe(`Type ${MIN_FIND_QUERY_CHARS}+ characters`);
    await stepFind(1);
    expect(askFrame, 'Enter on one character searches nothing either').not.toHaveBeenCalled();
  });

  it('takes the highlights away when the query shrinks below two characters', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 4, current: 1, capped: false });
    openFind('content');
    await typeQuery('ap');
    await typeQuery('a');
    expect(tellFrame).toHaveBeenCalledWith(frame, 'find', { clear: true });
    expect(findResult.value).toEqual({ status: 'too-short' });
  });

  it('does nothing on a reload while the bar is shut', async () => {
    await rerunFindOn('content');
    expect(askFrame).not.toHaveBeenCalled();
  });
});

describe('the find bar over a text preview', () => {
  function showPreview(html: string) {
    panelOverlay.value = file('artifacts/notes.md');
    document.body.innerHTML = `<div class="file-preview-frame-body">${html}</div><aside>apple outside</aside>`;
  }

  it('searches the preview itself, and nothing outside it', async () => {
    showPreview('<p>apple pie</p><p>green apple</p>');
    openFind('content');
    await typeQuery('apple');
    expect(findResult.value).toEqual({ status: 'found', total: 2, current: 1, capped: false });
    expect(askFrame).not.toHaveBeenCalled();
  });

  it('leaves the repo preview\'s file tree out', async () => {
    panelOverlay.value = file('artifacts/notes.md');
    document.body.innerHTML = `<div class="file-preview-frame-body">
      <nav>apple.md</nav><div class="repo-preview-split-main"><p>apple</p></div></div>`;
    openFind('content');
    await typeQuery('apple');
    expect(findResult.value).toMatchObject({ total: 1 });
  });

  it('scrolls a match out from under the open bar', async () => {
    showPreview('<p>apple</p>');
    document.body.insertAdjacentHTML('afterbegin', '<div class="file-preview-frame"><div class="find-bar-slot"></div></div>');
    const bar = document.querySelector<HTMLElement>('.find-bar-slot')!;
    bar.getBoundingClientRect = () => new DOMRect(0, 0, 10, 100);
    const scrollBy = vi.spyOn(window, 'scrollBy').mockImplementation(() => {});
    openFind('content');
    await typeQuery('apple');
    // The match's box is at the top of the viewport, under the bar's 100px.
    expect(scrollBy).toHaveBeenCalledWith(0, 0 - 100 - (window.innerHeight - 100 - 10) / 2);
    scrollBy.mockRestore();
  });

  it('never changes the preview\'s markup', async () => {
    showPreview('<p>one <b>two</b> one</p>');
    const before = document.body.innerHTML;
    openFind('content');
    await typeQuery('one');
    await stepFind(1);
    closeFind();
    expect(document.body.innerHTML).toBe(before);
  });
});

describe('a find a navigation asked for', () => {
  const PATH = 'artifacts/notes.md';
  const SCOPE = fileFindScope(PATH);

  /** The source view of a file, one `.code-line` per line, as `LineNumberedCode` draws it. */
  function showSource(lines: string[]) {
    panelOverlay.value = file(PATH);
    filePreviewSource.value = true;
    document.body.innerHTML = `<div class="file-preview-frame-body"><pre>${lines
      .map((text, i) => `<div class="code-line" data-line="${i + 1}"><span class="line-content">${text}</span></div>`)
      .join('')}</pre></div>`;
  }

  beforeEach(() => {
    findRequest.value = null;
    lineScrollTarget.value = null;
  });

  it('opens on the file it was made for, with the query, from the cited line', async () => {
    showSource(['apple', 'pear', 'apple', 'apple']);
    lineScrollTarget.value = { path: PATH, line: 3 };
    requestFind(SCOPE, 'apple', 3);

    takeFindRequest('content', fileFindScope('artifacts/other.md'));
    expect(findSurface.value, 'another file\'s bar leaves it alone').toBeNull();

    takeFindRequest('content', SCOPE);
    expect(findSurface.value).toBe('content');
    expect(findQuery.value).toBe('apple');
    expect(findRequest.value, 'taken once').toBeNull();
    await vi.runAllTimersAsync();
    expect(findResult.value, 'nothing runs before the line lands').toEqual({ status: 'idle' });

    lineScrollTarget.value = null;
    await vi.runAllTimersAsync();
    expect(findResult.value).toEqual({ status: 'found', total: 3, current: 2, capped: false });
    await stepFind(1);
    expect(findResult.value).toMatchObject({ current: 3 });
  });

  it('ignores a step before the line lands, so the search still starts at the line', async () => {
    showSource(['apple', 'pear', 'apple', 'apple']);
    lineScrollTarget.value = { path: PATH, line: 3 };
    requestFind(SCOPE, 'apple', 3);
    takeFindRequest('content', SCOPE);

    await stepFind(1);
    expect(findResult.value, 'an early Enter searches nothing').toEqual({ status: 'idle' });

    lineScrollTarget.value = null;
    await vi.runAllTimersAsync();
    expect(findResult.value).toEqual({ status: 'found', total: 3, current: 2, capped: false });
    await stepFind(1);
    expect(findResult.value).toMatchObject({ current: 3 });
  });

  it('runs at once when the line has already landed', async () => {
    showSource(['apple', 'apple']);
    requestFind(SCOPE, 'apple', 2);
    takeFindRequest('content', SCOPE);
    await vi.runAllTimersAsync();
    expect(findResult.value).toMatchObject({ total: 2, current: 2 });
  });

  it('takes focus with a fine pointer, so Enter steps through the matches', () => {
    showSource(['apple']);
    const focusRequest = findFocusRequest.value;
    requestFind(SCOPE, 'apple', 1);
    takeFindRequest('content', SCOPE);
    expect(findFocusRequest.value).toBe(focusRequest + 1);
  });

  it('takes no focus with a finger, so a phone keeps its keyboard down', () => {
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
    try {
      showSource(['apple']);
      const focusRequest = findFocusRequest.value;
      requestFind(SCOPE, 'apple', 1);
      takeFindRequest('content', SCOPE);
      expect(findFocusRequest.value).toBe(focusRequest);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('asks for nothing with a query too short to search', () => {
    requestFind(SCOPE, 'a'.repeat(MIN_FIND_QUERY_CHARS - 1), 1);
    expect(findRequest.value).toBeNull();
  });

  it('never runs on a bar closed before the line landed', async () => {
    showSource(['apple']);
    lineScrollTarget.value = { path: PATH, line: 1 };
    requestFind(SCOPE, 'apple', 1);
    takeFindRequest('content', SCOPE);
    closeFind();
    lineScrollTarget.value = null;
    await vi.runAllTimersAsync();
    expect(findSurface.value).toBeNull();
    expect(findResult.value).toEqual({ status: 'idle' });
  });
});

describe('one session at a time', () => {
  it('opening the bar on another surface closes the first', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 1, current: 1, capped: false });
    openFind('content');
    await typeQuery('apple');
    openFind('thread');
    expect(tellFrame).toHaveBeenCalledWith(frame, 'find', { clear: true });
    expect(findSurface.value).toBe('thread');
    expect(findQuery.value).toBe('');
  });
});

describe('the Mod+F action', () => {
  it('opens the bar on the focused pane, and closes an open bar like Escape', async () => {
    vi.mocked(askFrame).mockResolvedValue({ total: 1, current: 1, capped: false });
    toggleFocusedFind();
    expect(findSurface.value).toBe('content');
    await typeQuery('apple');
    toggleFocusedFind();
    expect(findSurface.value).toBeNull();
    expect(tellFrame, 'the highlights go with it').toHaveBeenCalledWith(frame, 'find', { clear: true });
  });

  it('closes a bar open on the other pane', () => {
    openFind('thread');
    toggleFocusedFind();
    expect(findSurface.value).toBeNull();
  });
});

describe('findStatusText', () => {
  it('reads the count the way a browser find bar does', () => {
    expect(findStatusText({ status: 'idle' })).toBe('');
    expect(findStatusText({ status: 'found', total: 0, current: 0, capped: false })).toBe('No matches');
    expect(findStatusText({ status: 'found', total: 12, current: 3, capped: false })).toBe('3 of 12');
    expect(findStatusText({ status: 'found', total: 2000, current: 1, capped: true })).toBe('1 of 2000+');
    // The transcript counts first, and moves only on a step.
    expect(findStatusText({ status: 'found', total: 3, current: 0, capped: false })).toBe('3 matches');
    expect(findStatusText({ status: 'found', total: 1, current: 0, capped: false })).toBe('1 match');
  });
});
