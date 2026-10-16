// @vitest-environment jsdom
/**
 * The drawn caret (ADR 0317): when it runs, and what it shows.
 *
 * jsdom lays nothing out, so position is the browser e2e's job
 * (`e2e/theme-caret-fallback.spec.ts`). These tests pin the decision and the
 * controller's reaction to focus, selection, composition and theme switches.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { attachDrawnCaret, drawnCaretShape } from './drawnCaret';

const TOKEN = '--part-composer-text-caret-shape';

describe('drawnCaretShape', () => {
  it('draws block and underscore where the browser has no caret-shape', () => {
    expect(drawnCaretShape('block', false, false)).toBe('block');
    expect(drawnCaretShape('underscore', false, false)).toBe('underscore');
  });

  it('leaves the caret to the browser where it draws caret-shape', () => {
    expect(drawnCaretShape('block', true, false)).toBeNull();
    expect(drawnCaretShape('underscore', true, false)).toBeNull();
  });

  it('draws nothing for auto, bar or an unset part', () => {
    for (const token of ['auto', 'bar', '', 'ibeam']) {
      expect(drawnCaretShape(token, false, false), token).toBeNull();
    }
  });

  it('draws nothing inside a protected surface', () => {
    expect(drawnCaretShape('block', false, true)).toBeNull();
  });
});

describe('attachDrawnCaret', () => {
  let textarea: HTMLTextAreaElement;
  let detach: () => void;

  const frame = () => new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
  const overlay = () => textarea.parentElement!.querySelector<HTMLElement>('.drawn-caret');
  const cursor = () => overlay()?.querySelector<HTMLElement>('.drawn-caret-cursor') ?? null;
  const shows = () => overlay() !== null && !overlay()!.hidden;
  const hidesNative = () => textarea.hasAttribute('data-drawn-caret');

  function setTheme(shape: string | null) {
    if (shape === null) document.documentElement.style.removeProperty(TOKEN);
    else document.documentElement.style.setProperty(TOKEN, shape);
  }

  beforeEach(() => {
    globalThis.ResizeObserver = class {
      observe() {}
      disconnect() {}
    } as unknown as typeof ResizeObserver;
    document.body.innerHTML = '<div class="prompt-row"><textarea placeholder="Post a follow up"></textarea></div>';
    textarea = document.querySelector('textarea')!;
  });

  afterEach(() => {
    detach?.();
    setTheme(null);
    document.body.innerHTML = '';
  });

  it('adds no DOM and no listeners where the browser draws caret-shape', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: true });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(overlay()).toBeNull();
    expect(hidesNative()).toBe(false);
  });

  it('draws a block over the character at the caret while focused', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.value = 'ls -la';
    textarea.focus();
    textarea.setSelectionRange(4, 4);
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(shows()).toBe(true);
    expect(hidesNative()).toBe(true);
    expect(overlay()!.getAttribute('aria-hidden')).toBe('true');
    expect(cursor()!.dataset.shape).toBe('block');
    expect(cursor()!.textContent).toBe('l');
    expect(overlay()!.textContent).toBe('ls -la');
  });

  it('takes the placeholder character on an empty composer, and none at the end of the text', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(cursor()!.textContent).toBe('P');

    textarea.value = 'ls';
    textarea.setSelectionRange(2, 2);
    textarea.dispatchEvent(new Event('input'));
    await frame();
    expect(cursor()!.textContent).toBe('');
  });

  it('keeps a whole emoji under the block', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.value = 'a👍🏽b';
    textarea.focus();
    textarea.setSelectionRange(1, 1);
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(cursor()!.textContent).toBe('👍🏽');

    // A caret set inside the cluster never repeats text before it.
    textarea.setSelectionRange(3, 3);
    document.dispatchEvent(new Event('selectionchange'));
    await frame();
    expect(overlay()!.textContent).toBe('a👍🏽b');
  });

  it('draws an underscore', async () => {
    setTheme('underscore');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(cursor()!.dataset.shape).toBe('underscore');
  });

  it('hides on blur and on a range selection, and keeps the native caret hidden', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.value = 'ls -la';
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(shows()).toBe(true);

    textarea.setSelectionRange(0, 3);
    document.dispatchEvent(new Event('selectionchange'));
    await frame();
    expect(shows()).toBe(false);
    expect(hidesNative()).toBe(true);

    textarea.setSelectionRange(3, 3);
    textarea.blur();
    textarea.dispatchEvent(new Event('blur'));
    await frame();
    expect(shows()).toBe(false);
  });

  it('gives the caret back to the browser during IME composition', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(shows()).toBe(true);

    textarea.dispatchEvent(new CompositionEvent('compositionstart'));
    await frame();
    expect(shows()).toBe(false);
    expect(hidesNative()).toBe(false);

    textarea.dispatchEvent(new CompositionEvent('compositionend'));
    await frame();
    expect(shows()).toBe(true);
    expect(hidesNative()).toBe(true);
  });

  it('follows a theme switch at once, both ways', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(shows()).toBe(true);

    setTheme(null);
    await Promise.resolve();
    await frame();
    expect(overlay()).toBeNull();
    expect(hidesNative()).toBe(false);

    setTheme('underscore');
    await Promise.resolve();
    await frame();
    expect(cursor()!.dataset.shape).toBe('underscore');
    expect(hidesNative()).toBe(true);
  });

  it('never attaches inside a protected surface', async () => {
    setTheme('block');
    document.body.innerHTML = '<div class="protected-surface"><div class="prompt-row"><textarea></textarea></div></div>';
    textarea = document.querySelector('textarea')!;
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    expect(overlay()).toBeNull();
    expect(hidesNative()).toBe(false);
  });

  it('removes everything on detach', async () => {
    setTheme('block');
    detach = attachDrawnCaret(textarea, { nativeCaretShape: false });
    textarea.focus();
    textarea.dispatchEvent(new Event('focus'));
    await frame();
    detach();
    expect(overlay()).toBeNull();
    expect(hidesNative()).toBe(false);

    textarea.dispatchEvent(new Event('input'));
    await frame();
    expect(overlay()).toBeNull();
  });
});
