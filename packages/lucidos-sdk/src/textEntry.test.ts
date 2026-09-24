// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { defaultAutocorrect, isTextEntryField, resolveAutocorrect } from './textEntry';

describe('defaultAutocorrect', () => {
  it('is on, with no platform to consult', () => {
    expect(defaultAutocorrect()).toBe(true);
  });
});

describe('resolveAutocorrect', () => {
  it('lets a stored value win on any client', () => {
    expect(resolveAutocorrect('true')).toBe(true);
    expect(resolveAutocorrect('false')).toBe(false);
  });

  it('reads anything else as unset, which falls to the default of on', () => {
    for (const raw of [undefined, null, '', 'off', 'TRUE']) {
      expect(resolveAutocorrect(raw)).toBe(true);
    }
  });
});

describe('isTextEntryField', () => {
  const make = (html: string) => {
    const host = document.createElement('div');
    host.innerHTML = html;
    return host.firstElementChild as Element;
  };

  it('takes a textarea and every text-ish input', () => {
    expect(isTextEntryField(make('<textarea></textarea>'))).toBe(true);
    for (const type of ['text', 'search', 'email', 'url', 'tel', 'password', 'date']) {
      expect(isTextEntryField(make(`<input type="${type}">`))).toBe(true);
    }
    // No type attribute is a text input.
    expect(isTextEntryField(make('<input>'))).toBe(true);
  });

  it('skips inputs that take no typing, and anything that is not a field', () => {
    for (const type of ['button', 'submit', 'checkbox', 'radio', 'range', 'file', 'hidden']) {
      expect(isTextEntryField(make(`<input type="${type}">`))).toBe(false);
    }
    expect(isTextEntryField(make('<div contenteditable></div>'))).toBe(false);
    expect(isTextEntryField(make('<select></select>'))).toBe(false);
  });
});
