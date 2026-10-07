// @vitest-environment jsdom
import { FLAG_OFF_VALUES, FLAG_ON_VALUES } from './generated/preference-catalog';
import { describe, it, expect } from 'vitest';
import {
  defaultAutocorrect, isKeyCodeTextInsertion, isTextEntryField, resolveAutocorrect,
} from './textEntry';

describe('resolveAutocorrect', () => {
  it('lets every spelling the engine reads win on any client', () => {
    for (const raw of FLAG_ON_VALUES) expect(resolveAutocorrect(` ${raw.toUpperCase()} `)).toBe(true);
    for (const raw of FLAG_OFF_VALUES) expect(resolveAutocorrect(` ${raw.toUpperCase()} `)).toBe(false);
  });

  it('reads anything else as unset, which falls to the default', () => {
    for (const raw of [undefined, null, '', 'maybe']) {
      expect(resolveAutocorrect(raw)).toBe(defaultAutocorrect());
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

// Spelled as codepoints so no unprintable character sits in this file.
const char = (code: number) => String.fromCodePoint(code);

describe('isKeyCodeTextInsertion', () => {
  it('refuses the control codes the desktop app types for the four arrows', () => {
    // Left, right, up, down, as a macOS key event carries them.
    for (const code of [0x1c, 0x1d, 0x1e, 0x1f]) {
      expect(isKeyCodeTextInsertion('insertText', char(code))).toBe(true);
    }
  });

  it('refuses a held arrow, which arrives as one character per repeat', () => {
    expect(isKeyCodeTextInsertion('insertText', char(0x1d))).toBe(true);
    expect(isKeyCodeTextInsertion('insertText', char(0x1d).repeat(3))).toBe(true);
  });

  it('refuses the other control codes and DEL', () => {
    expect(isKeyCodeTextInsertion('insertText', char(0x00))).toBe(true);
    expect(isKeyCodeTextInsertion('insertText', char(0x1b))).toBe(true);
    expect(isKeyCodeTextInsertion('insertText', char(0x7f))).toBe(true);
  });

  it('refuses the AppKit function-key constants', () => {
    expect(isKeyCodeTextInsertion('insertText', char(0xf703))).toBe(true);
    expect(isKeyCodeTextInsertion('insertText', char(0xf72d))).toBe(true);
    expect(isKeyCodeTextInsertion('insertText', char(0xf747))).toBe(true);
  });

  it('lets tab and both line breaks through, since they are text', () => {
    for (const text of ['\t', '\n', '\r', '\r\n']) {
      expect(isKeyCodeTextInsertion('insertText', text)).toBe(false);
    }
  });

  it('leaves a private-use glyph above the assigned constants alone', () => {
    expect(isKeyCodeTextInsertion('insertText', char(0xf7ff))).toBe(false);
  });

  it('leaves ordinary typing and an emoji alone', () => {
    expect(isKeyCodeTextInsertion('insertText', 'a')).toBe(false);
    expect(isKeyCodeTextInsertion('insertText', 'a slack adapter?')).toBe(false);
    expect(isKeyCodeTextInsertion('insertText', char(0x1f600))).toBe(false);
  });

  it('leaves text that merely contains a key code alone', () => {
    expect(isKeyCodeTextInsertion('insertText', `adapter?${char(0x1d)}`)).toBe(false);
  });

  it('ignores a deletion, a history step, and a paste with no data', () => {
    expect(isKeyCodeTextInsertion('deleteContentBackward', null)).toBe(false);
    expect(isKeyCodeTextInsertion('historyUndo', null)).toBe(false);
    expect(isKeyCodeTextInsertion('insertFromPaste', null)).toBe(false);
  });
});
