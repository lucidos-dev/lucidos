// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { holdSoftwareKeyboard, isOverlayField } from './softwareKeyboard';
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

// The header keeps still for these, since the screen behind an overlay must.
describe('isOverlayField', () => {
  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('counts the proxy and a field inside an overlay panel, and nothing else', () => {
    setCoarsePointer(true);
    holdSoftwareKeyboard();
    expect(isOverlayField(document.activeElement as Element)).toBe(true);

    document.body.innerHTML = '<div data-overlay-panel="overlay-1"><input id="in"></div><textarea id="out"></textarea>';
    expect(isOverlayField(document.querySelector('#in')!)).toBe(true);
    expect(isOverlayField(document.querySelector('#out')!)).toBe(false);
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
