// @vitest-environment jsdom
/** A touch menu opened mid-typing takes the on-screen keyboard for its filter
 *  box, and hands it back to the field as it closes. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { keyboardHolder, returnKeyboard } from '../keyboardHandoff';

const fingerPointer = (coarse: boolean) =>
  vi.stubGlobal('matchMedia', (query: string) => ({ matches: coarse && query === '(pointer: coarse)' }));

const added: HTMLElement[] = [];
function mount<K extends keyof HTMLElementTagNameMap>(tag: K, type?: string): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  if (type) el.setAttribute('type', type);
  document.body.appendChild(el);
  added.push(el);
  return el;
}

beforeEach(() => {
  vi.stubGlobal('innerWidth', 1280);
  fingerPointer(true);
});

afterEach(() => {
  added.splice(0).forEach((el) => el.remove());
  vi.unstubAllGlobals();
});

describe('keyboardHolder', () => {
  it('names the focused text field under a finger', () => {
    const prompt = mount('textarea');
    prompt.focus();
    expect(keyboardHolder()).toBe(prompt);
  });

  it('names none when no field holds the keyboard', () => {
    expect(keyboardHolder()).toBeNull();
  });

  it('names none for a focused control that opens no keyboard', () => {
    // A checkbox holds focus with no keyboard up. Taking it over would raise
    // one over the list the user opened to tap.
    mount('input', 'checkbox').focus();
    expect(keyboardHolder()).toBeNull();
  });

  it('names none under a mouse in a wide window, where the trigger takes the keys', () => {
    fingerPointer(false);
    mount('textarea').focus();
    expect(keyboardHolder()).toBeNull();
  });

  it('names the field in a phone-width window, where no trigger takes focus', () => {
    // The touch layout leaves the trigger unfocused, so without the handoff a
    // keystroke would land in the prompt.
    fingerPointer(false);
    vi.stubGlobal('innerWidth', 600);
    const prompt = mount('textarea');
    prompt.focus();
    expect(keyboardHolder()).toBe(prompt);
  });
});

describe('returnKeyboard', () => {
  it('hands focus back while the menu still holds it', () => {
    const prompt = mount('textarea');
    const panel = mount('div');
    const filter = document.createElement('input');
    panel.appendChild(filter);
    filter.focus();

    expect(returnKeyboard(prompt, panel)).toBe(true);
    expect(document.activeElement).toBe(prompt);
  });

  it('hands focus back when the step that held it left nothing focused', () => {
    // A tier row replaces the filter box, so focus falls to the body.
    const prompt = mount('textarea');
    expect(returnKeyboard(prompt, mount('div'))).toBe(true);
    expect(document.activeElement).toBe(prompt);
  });

  it('leaves focus with a field outside the menu that claimed it', () => {
    const prompt = mount('textarea');
    const followUp = mount('input');
    followUp.focus();

    expect(returnKeyboard(prompt, mount('div'))).toBe(false);
    expect(document.activeElement).toBe(followUp);
  });

  it('does nothing for a field that has left the page', () => {
    const prompt = document.createElement('textarea');
    expect(returnKeyboard(prompt, mount('div'))).toBe(false);
  });
});
