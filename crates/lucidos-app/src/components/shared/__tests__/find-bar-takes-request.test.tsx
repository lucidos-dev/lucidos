// @vitest-environment jsdom
/**
 * A Search Everywhere text hit asks for a find on the file it opens. The
 * content pane's bar opens it once it shows that file. By then the scope
 * change has ended the previous file's session. The caret moves into the
 * field, unless a finger would raise a keyboard over the file.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { FindBar } from '../FindBar';
import { findQuery, findSurface, openFind, resetFind, setFindQuery } from '../../../store/actions/find-bar';
import { fileFindScope, requestFind } from '../../../store/actions/find-request';
import { findRequest } from '../../../store/store';

const OLD = fileFindScope('artifacts/old.md');
const NEW = fileFindScope('artifacts/new.md');

let host: HTMLElement | null = null;

function show(scope: string) {
  host ??= document.body.appendChild(document.createElement('div'));
  act(() => { render(<FindBar surface="content" scope={scope} placeholder="Find in file" />, host!); });
}

afterEach(() => {
  if (host) { act(() => { render(null, host!); }); host.remove(); host = null; }
  findRequest.value = null;
  resetFind();
});

describe('the find bar takes a find asked for its file', () => {
  it('waits for the bar to show the new file, then opens on the query there', () => {
    show(OLD);
    act(() => {
      openFind('content');
      setFindQuery('pear');
    });

    act(() => { requestFind(NEW, 'apple', 3); });
    expect(findQuery.value, 'the old file\'s bar leaves it alone').toBe('pear');

    show(NEW);
    expect(findSurface.value).toBe('content');
    expect(findQuery.value).toBe('apple');
    expect(document.querySelector<HTMLInputElement>('[data-role="find-input"]')!.value).toBe('apple');
    expect(findRequest.value).toBeNull();
  });

  it('puts the caret in the field with a fine pointer, so Enter steps through the matches', () => {
    show(OLD);
    act(() => { requestFind(OLD, 'apple', 1); });
    expect(findSurface.value).toBe('content');
    expect(document.activeElement).toBe(document.querySelector('[data-role="find-input"]'));
  });

  it('leaves the caret where it is with a finger', () => {
    vi.stubGlobal('matchMedia', (query: string) => ({ matches: query === '(pointer: coarse)' }));
    const outside = document.body.appendChild(document.createElement('button'));
    try {
      outside.focus();
      show(OLD);
      act(() => { requestFind(OLD, 'apple', 1); });
      expect(findSurface.value).toBe('content');
      expect(document.activeElement).toBe(outside);
    } finally {
      outside.remove();
      vi.unstubAllGlobals();
    }
  });
});
