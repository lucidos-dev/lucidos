// @vitest-environment jsdom
/**
 * Enter COMMITS an IME candidate, and must never also send.
 *
 * A Japanese, Chinese or Korean user types a phrase and presses Enter to accept
 * what the IME is offering. The browser dispatches that keydown before
 * `compositionend`. An Enter branch with no composition guard fired on it: the
 * composer sent half-converted text, and the title editor renamed the thread.
 * `preventDefault` also fought the IME's own commit.
 *
 * `isImeComposingKey` is the shared guard both handlers now take first. It is
 * tested here as a pure predicate, the way `shouldTypeToFocusPrompt` is tested
 * in `hooks/useKeyboardShortcuts.test.ts`. The composer's own handler is a
 * closure over component state, so a test cannot reach it. The rendered half
 * runs against `ThreadTitleEditor`, whose save is one mockable call.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';

vi.mock('../../../api/threads', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/threads')>()),
  renameThread: vi.fn(async () => {}),
  suggestTitle: vi.fn(async () => 'Suggested title'),
}));

import { isImeComposingKey } from '../PromptInput';
import { ThreadTitleEditor } from '../ThreadTitleEditor';
import { renameThread } from '../../../api/threads';

describe('isImeComposingKey', () => {
  it('reads the standard isComposing flag', () => {
    expect(isImeComposingKey({ isComposing: true, keyCode: 13 })).toBe(true);
  });

  it('reads the legacy 229 keyCode, which is all some browsers send', () => {
    expect(isImeComposingKey({ isComposing: false, keyCode: 229 })).toBe(true);
  });

  it('is false for an ordinary Enter, so sending still works', () => {
    expect(isImeComposingKey({ isComposing: false, keyCode: 13 })).toBe(false);
  });
});

describe('ThreadTitleEditor rename across an IME composition', () => {
  let host: HTMLDivElement;

  /** The desktop rename field. jsdom reports a 1024px width, so it is an input. */
  const field = (): HTMLInputElement => {
    const el = host.querySelector<HTMLInputElement>('input.thread-title-edit-input');
    expect(el, 'the rename field did not render').not.toBeNull();
    return el as HTMLInputElement;
  };

  /** Open the editor and type. Typing is what arms the dirty flag `save` reads. */
  const typeTitle = (text: string): HTMLInputElement => {
    const el = field();
    el.focus();
    el.value = text;
    el.dispatchEvent(new Event('input', { bubbles: true }));
    return el;
  };

  const press = (el: HTMLElement, init: KeyboardEventInit): KeyboardEvent => {
    const ev = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
    el.dispatchEvent(ev);
    return ev;
  };

  beforeEach(() => {
    vi.mocked(renameThread).mockClear();
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<ThreadTitleEditor threadId="t1" title="old title" />, host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('does NOT rename on the Enter that commits a candidate', () => {
    const ev = press(typeTitle('にほんご'), { key: 'Enter', isComposing: true });
    expect(renameThread).not.toHaveBeenCalled();
    expect(ev.defaultPrevented, 'preventDefault fights the IME commit').toBe(false);
  });

  it('does NOT rename when only the legacy 229 keyCode marks the composition', () => {
    press(typeTitle('にほんご'), { key: 'Enter', keyCode: 229 });
    expect(renameThread).not.toHaveBeenCalled();
  });

  it('still renames on an ordinary Enter', () => {
    const ev = press(typeTitle('a new title'), { key: 'Enter', isComposing: false });
    expect(renameThread).toHaveBeenCalledWith('t1', 'a new title');
    expect(ev.defaultPrevented).toBe(true);
  });

  // Escape is gated with Enter, deliberately. Mid-composition it belongs to the
  // IME, which cancels the candidate. The editor answers the next one.
  it('leaves Escape to the IME while composing, and takes it otherwise', () => {
    const el = typeTitle('にほんご');
    expect(press(el, { key: 'Escape', isComposing: true }).defaultPrevented).toBe(false);
    expect(press(el, { key: 'Escape', isComposing: false }).defaultPrevented).toBe(true);
  });
});
