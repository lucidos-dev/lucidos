// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  clearFind, collectPageText, find, matchSpans, MAX_FIND_MATCHES, queryPattern, serveFind, spanToRange, stepIndex,
} from './find';

// jsdom has no layout. Every range sits at the top of the viewport, which is
// what a real one-screen page reports too.
Range.prototype.getBoundingClientRect = () => new DOMRect(0, 0, 10, 10);

describe('queryPattern', () => {
  it('matches without regard to case', () => {
    expect('Hello WORLD'.match(queryPattern('world')!)?.[0]).toBe('WORLD');
  });

  it('treats the query as text, never as a regex', () => {
    expect('a.b axb'.match(queryPattern('a.b')!)).toEqual(['a.b']);
  });

  it('lets a space in the query match the page\'s line breaks and indents', () => {
    expect('foo\n    bar'.match(queryPattern('foo bar')!)?.[0]).toBe('foo\n    bar');
  });

  it('has nothing to find for a blank query', () => {
    expect(queryPattern('   ')).toBeNull();
  });
});

describe('matchSpans', () => {
  it('reports start and end of every match', () => {
    expect(matchSpans('abcabc', queryPattern('bc')!, 10)).toEqual([[1, 3], [4, 6]]);
  });

  it('stops at the limit', () => {
    expect(matchSpans('aaaa', queryPattern('a')!, 2)).toHaveLength(2);
  });
});

describe('stepIndex', () => {
  it('wraps past the last match to the first, and back', () => {
    expect(stepIndex(2, 1, 3)).toBe(0);
    expect(stepIndex(0, -1, 3)).toBe(2);
    expect(stepIndex(1, 1, 3)).toBe(2);
  });

  it('stays at 0 with nothing matched', () => {
    expect(stepIndex(0, 1, 0)).toBe(0);
  });
});

describe('find over a document', () => {
  beforeEach(() => {
    clearFind();
    document.body.innerHTML = '';
  });

  afterEach(() => {
    clearFind();
  });

  it('counts the visible text only', () => {
    document.body.innerHTML = `
      <p>apple pie</p>
      <script>const apple = 1;</script>
      <style>.apple {}</style>
      <textarea>apple</textarea>
      <template><p>apple</p></template>
      <p>green <b>apple</b></p>`;
    expect(find('apple')).toEqual({ total: 2, current: 1, capped: false });
  });

  it('matches across inline elements, but not from one block into the next', () => {
    document.body.innerHTML = '<p>red <em>apple</em></p><p>pie</p>';
    expect(find('red apple').total).toBe(1);
    expect(find('apple pie').total).toBe(0);
  });

  it('reads a line break as whitespace, never as nothing', () => {
    document.body.innerHTML = '<p>Oslo<br>Norway</p>';
    expect(find('oslonorway').total).toBe(0);
    expect(find('oslo norway').total).toBe(1);
  });

  it('reads a document that was parsed but never shown, telling blocks by tag', () => {
    const parsed = document.implementation.createHTMLDocument('');
    parsed.body.innerHTML = '<p>red <em>apple</em></p><ul><li>pie</li></ul>';
    expect(collectPageText(parsed.body, 'markup').text).toBe('red apple\u0000pie');
  });

  it('never changes the app\'s markup, through a find, a step and a clear', () => {
    document.body.innerHTML = '<div id="x"><p>one <b>two</b> one</p></div>';
    const before = document.body.innerHTML;
    find('one');
    find('one', 1);
    serveFind({ clear: true });
    expect(document.body.innerHTML).toBe(before);
  });

  it('steps through the matches and wraps at both ends', () => {
    document.body.innerHTML = '<p>a</p><p>a</p><p>a</p>';
    expect(find('a').current).toBe(1);
    expect(find('a', 1).current).toBe(2);
    expect(find('a', 1).current).toBe(3);
    expect(find('a', 1).current).toBe(1);
    expect(find('a', -1).current).toBe(3);
  });

  it('scrolls the page by the match\'s own box, centring one below the fold', () => {
    const below = window.innerHeight + 500;
    const rect = vi.spyOn(Range.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, below, 10, 20));
    const scrollBy = vi.spyOn(window, 'scrollBy').mockImplementation(() => {});
    document.body.innerHTML = '<p>a</p>';
    find('a');
    expect(scrollBy).toHaveBeenCalledWith(0, below - (window.innerHeight - 20) / 2);
    rect.mockRestore();
    scrollBy.mockRestore();
  });

  it('leaves the page alone when the match already shows', () => {
    const scrollBy = vi.spyOn(window, 'scrollBy').mockImplementation(() => {});
    document.body.innerHTML = '<p>a</p>';
    find('a');
    expect(scrollBy).not.toHaveBeenCalled();
    scrollBy.mockRestore();
  });

  it('keeps its place when the same query runs again, as on a frame reload', () => {
    document.body.innerHTML = '<p>a</p><p>a</p><p>a</p>';
    find('a');
    find('a', 1);
    expect(find('a').current).toBe(2);
  });

  it('starts over on a new query', () => {
    document.body.innerHTML = '<p>ab</p><p>ab</p>';
    find('a');
    find('a', 1);
    expect(find('ab').current).toBe(1);
  });

  it('reports no match as zero of zero', () => {
    document.body.innerHTML = '<p>apple</p>';
    expect(find('pear')).toEqual({ total: 0, current: 0, capped: false });
  });

  it('stops counting at the cap, and says so', () => {
    document.body.innerHTML = `<p>${'x'.repeat(MAX_FIND_MATCHES + 5)}</p>`;
    expect(find('x')).toEqual({ total: MAX_FIND_MATCHES, current: 1, capped: true });
  });

  it('maps a match that spans nodes back to the right characters', () => {
    document.body.innerHTML = '<p>red <em>apple</em> pie</p>';
    const page = collectPageText();
    expect(page.text).toBe('red apple pie');
    const range = spanToRange(page, matchSpans(page.text, queryPattern('d app')!, 1)[0]);
    expect(range.toString()).toBe('d app');
    expect(range.startContainer.nodeValue).toBe('red ');
    expect(range.endContainer.nodeValue).toBe('apple');
  });

  it('shows the current match as the selection where highlights are missing', () => {
    document.body.innerHTML = '<p>one two one</p>';
    find('one', undefined);
    find('one', 1);
    expect(window.getSelection()?.getRangeAt(0).startOffset).toBe(8);
    clearFind();
    expect(window.getSelection()?.rangeCount).toBe(0);
  });
});

describe('find with the highlight API and sdk-iframe.css', () => {
  const highlights = new Map<string, { ranges: Range[] }>();

  beforeEach(() => {
    highlights.clear();
    vi.stubGlobal('CSS', { highlights });
    vi.stubGlobal('Highlight', class { ranges: Range[]; constructor(...r: Range[]) { this.ranges = r; } });
    document.documentElement.style.setProperty('--find-highlights', 'styled');
    clearFind();
  });

  afterEach(() => {
    clearFind();
    document.documentElement.style.removeProperty('--find-highlights');
    vi.unstubAllGlobals();
  });

  it('highlights every match, marks the current one, and leaves the selection alone', () => {
    document.body.innerHTML = '<p>one two one</p>';
    find('one');
    find('one', 1);
    expect(highlights.get('lucidos-find')?.ranges).toHaveLength(2);
    expect(highlights.get('lucidos-find-current')?.ranges[0].startOffset).toBe(8);
    expect(window.getSelection()?.rangeCount ?? 0).toBe(0);
  });

  it('takes both highlights away on clear', () => {
    document.body.innerHTML = '<p>one</p>';
    find('one');
    serveFind({ clear: true });
    expect(highlights.size).toBe(0);
  });

  it('falls back to the selection in an app that loads no sdk-iframe.css', () => {
    document.documentElement.style.removeProperty('--find-highlights');
    document.body.innerHTML = '<p>one</p>';
    find('one');
    expect(highlights.size).toBe(0);
    expect(window.getSelection()?.toString()).toBe('one');
  });
});

describe('serveFind', () => {
  it('refuses a request with no query', () => {
    expect(() => serveFind({})).toThrow(/query/);
    expect(() => serveFind(null)).toThrow(/query/);
  });

  it('ignores a step that is not one forward or back', () => {
    document.body.innerHTML = '<p>a</p><p>a</p><p>a</p>';
    clearFind();
    serveFind({ query: 'a' });
    expect(serveFind({ query: 'a', step: 7 }).current).toBe(1);
  });
});
