import { describe, it, expect } from 'vitest';

// Stub HTMLElement before importing — match scroll.test.ts pattern.
if (typeof globalThis.HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class HTMLElement {
    tagName = '';
    isContentEditable = false;
    contentEditable = 'inherit';
    // Match HTMLInputElement.type's real normalization — defaults to 'text',
    // never empty string. opensSoftwareKeyboard relies on this invariant.
    type = 'text';
  };
}

import { isTextInput, opensSoftwareKeyboard } from './dom';

function input(type: string) {
  const el = new (globalThis as any).HTMLElement();
  el.tagName = 'INPUT';
  el.type = type;
  return el;
}

function tag(name: string, contentEditable = false) {
  const el = new (globalThis as any).HTMLElement();
  el.tagName = name.toUpperCase();
  el.isContentEditable = contentEditable;
  return el;
}

describe('isTextInput', () => {
  it('returns true for INPUT, TEXTAREA, SELECT, contenteditable', () => {
    expect(isTextInput(input('text'))).toBe(true);
    expect(isTextInput(input('range'))).toBe(true);
    expect(isTextInput(input('checkbox'))).toBe(true);
    expect(isTextInput(tag('textarea'))).toBe(true);
    expect(isTextInput(tag('select'))).toBe(true);
    expect(isTextInput(tag('div', true))).toBe(true);
    expect(isTextInput(tag('div'))).toBe(false);
    expect(isTextInput(null)).toBe(false);
  });
});

describe('opensSoftwareKeyboard', () => {
  it('returns true for textarea and contenteditable', () => {
    expect(opensSoftwareKeyboard(tag('textarea'))).toBe(true);
    expect(opensSoftwareKeyboard(tag('div', true))).toBe(true);
  });

  it('returns true for keyboard-opening input types', () => {
    for (const t of ['text', 'search', 'email', 'password', 'tel', 'url', 'number']) {
      expect(opensSoftwareKeyboard(input(t)), `type=${t}`).toBe(true);
    }
  });

  it('returns true for default input (no type attribute = text)', () => {
    const el = new (globalThis as any).HTMLElement();
    el.tagName = 'INPUT';
    // type defaults to 'text' in the stub (matches real HTMLInputElement.type)
    expect(opensSoftwareKeyboard(el)).toBe(true);
  });

  it('returns false for inputs that do NOT open the OS keyboard', () => {
    for (const t of ['range', 'checkbox', 'radio', 'button', 'submit', 'reset',
                     'color', 'file', 'date', 'time', 'hidden']) {
      expect(opensSoftwareKeyboard(input(t)), `type=${t}`).toBe(false);
    }
  });

  it('returns false for SELECT (opens picker, not keyboard)', () => {
    expect(opensSoftwareKeyboard(tag('select'))).toBe(false);
  });

  it('returns false for non-input elements and null', () => {
    expect(opensSoftwareKeyboard(tag('div'))).toBe(false);
    expect(opensSoftwareKeyboard(tag('button'))).toBe(false);
    expect(opensSoftwareKeyboard(null)).toBe(false);
  });
});
