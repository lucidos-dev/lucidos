// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

vi.mock('./app-bridge', () => ({ askFrame: vi.fn(), tellFrame: vi.fn() }));
vi.mock('./apps', () => ({ getVisibleAppFrame: vi.fn() }));
// jsdom lays nothing out, so every element would read as hidden.
vi.mock('../../components/chat/scrollState', () => ({ isElementVisible: () => true }));

import { askFrame, tellFrame } from './app-bridge';
import { getVisibleAppFrame } from './apps';
import {
  FIND_TIMEOUT_MS, closeFind, contentFindKind, findQuery, findResult, findStatusText, findSurface,
  focusedFindSurface, openFind, parseFindReply, rerunFindOn, resetFind, setFindQuery, stepFind,
} from './find-bar';
import { filePreviewEditing, filePreviewSource, focusedPane, panelOverlay } from '../store';

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
    await typeQuery('a');
    await typeQuery('ab');
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
