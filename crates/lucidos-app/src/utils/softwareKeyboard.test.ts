// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { holdSoftwareKeyboard } from './softwareKeyboard';
import { promptState, showPrompt } from '../store/store';

function setCoarsePointer(coarse: boolean): void {
  window.matchMedia = vi.fn((query: string) => ({
    matches: coarse && query === '(pointer: coarse)',
  })) as unknown as typeof window.matchMedia;
}

describe('holdSoftwareKeyboard', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    document.body.innerHTML = '';
  });

  it('focuses an off-screen input on a finger, then removes it', () => {
    setCoarsePointer(true);
    holdSoftwareKeyboard();
    const proxy = document.activeElement as HTMLElement;
    expect(proxy.tagName).toBe('INPUT');
    expect(proxy.isConnected).toBe(true);
    vi.runAllTimers();
    expect(proxy.isConnected).toBe(false);
  });

  // A fine pointer has no software keyboard, and the proxy would replace the
  // real opener that a dialog hands focus back to on close.
  it('leaves focus alone on a fine pointer', () => {
    setCoarsePointer(false);
    const opener = document.createElement('button');
    document.body.appendChild(opener);
    opener.focus();
    holdSoftwareKeyboard();
    expect(document.activeElement).toBe(opener);
  });
});

describe('showPrompt on a finger', () => {
  beforeEach(() => {
    promptState.value = { visible: false, message: '' };
    setCoarsePointer(true);
  });
  afterEach(() => {
    document.body.innerHTML = '';
  });

  // The dialog's input only focuses after a render, outside the tap, and iOS
  // raises no keyboard there unless a field already holds it.
  it('holds the keyboard within the call that opened it', () => {
    void showPrompt('Give this thread a new name.');
    expect((document.activeElement as HTMLElement).tagName).toBe('INPUT');
  });
});
