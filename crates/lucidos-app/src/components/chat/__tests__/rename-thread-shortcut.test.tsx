// @vitest-environment jsdom
/**
 * The Rename thread shortcut (F2) opens the visible title editor, and does
 * nothing when no thread header is on screen.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/threads')>()),
  renameThread: vi.fn(async () => {}),
  suggestTitle: vi.fn(async () => 'Suggested title'),
}));

// jsdom lays nothing out, so every box measures zero. Answer as a browser would
// for a header that is on screen.
let visible = true;
vi.mock('../scrollState', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../scrollState')>()),
  isElementVisible: () => visible,
}));

import { ThreadTitleEditor, focusThreadTitleEditor } from '../ThreadTitleEditor';

let host: HTMLDivElement | null = null;

afterEach(() => {
  if (host) {
    render(null, host);
    host.remove();
    host = null;
  }
});

describe('focusThreadTitleEditor', () => {
  it('opens the title editor of the thread on screen', () => {
    host = document.createElement('div');
    document.body.appendChild(host);
    const target = host;
    act(() => render(<ThreadTitleEditor threadId="t1" title="old title" />, target));

    act(() => focusThreadTitleEditor());

    expect(host.querySelector('.thread-title-edit.is-editing')).not.toBeNull();
    expect(document.activeElement?.getAttribute('data-role')).toBe('thread-title-input');
  });

  it('opens the laid-out copy while the pane animates open, never the hidden one', () => {
    // The pane is still zero wide, and the other layout's copy is display:none.
    // Only the laid-out copy reports a client rect, which is what can take focus.
    visible = false;
    host = document.createElement('div');
    document.body.appendChild(host);
    const target = host;
    act(() => render(
      <>
        <div data-copy="header"><ThreadTitleEditor threadId="t1" title="old title" /></div>
        <div data-copy="hidden"><ThreadTitleEditor threadId="t1" title="old title" /></div>
      </>,
      target,
    ));
    const header = host.querySelector<HTMLElement>('[data-copy="header"] [data-role="thread-title-input"]');
    header!.getClientRects = () => [new DOMRect(0, 0, 0, 20)] as unknown as DOMRectList;

    act(() => focusThreadTitleEditor());

    expect(host.querySelector('[data-copy="header"] .thread-title-edit.is-editing')).not.toBeNull();
    expect(host.querySelector('[data-copy="hidden"] .thread-title-edit.is-editing')).toBeNull();
    visible = true;
  });

  it('does nothing when no thread header is on screen', () => {
    focusThreadTitleEditor();
    expect(document.activeElement).toBe(document.body);
  });
});
