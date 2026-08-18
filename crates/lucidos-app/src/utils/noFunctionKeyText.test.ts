import { describe, it, expect, beforeAll } from 'vitest';
import { isFunctionKeyTextInsertion, installNoFunctionKeyText } from './noFunctionKeyText';

// The characters AppKit puts on an unhandled function key. Spelling them as
// codepoints keeps this file readable in a diff.
const RIGHT_ARROW = String.fromCodePoint(0xf703);
const LEFT_ARROW = String.fromCodePoint(0xf702);
const PAGE_DOWN = String.fromCodePoint(0xf72d);
const MODE_SWITCH = String.fromCodePoint(0xf747);
// A glyph a custom font squats on, above the last constant AppKit assigns. No
// key event can carry it, so the guard has no business refusing it.
const FONT_GLYPH = String.fromCodePoint(0xf7ff);

describe('isFunctionKeyTextInsertion', () => {
  it('refuses a lone function-key character', () => {
    expect(isFunctionKeyTextInsertion('insertText', RIGHT_ARROW)).toBe(true);
    expect(isFunctionKeyTextInsertion('insertText', LEFT_ARROW)).toBe(true);
    expect(isFunctionKeyTextInsertion('insertText', PAGE_DOWN)).toBe(true);
    expect(isFunctionKeyTextInsertion('insertText', MODE_SWITCH)).toBe(true);
  });

  it('leaves a private-use glyph above the assigned constants alone', () => {
    expect(isFunctionKeyTextInsertion('insertText', FONT_GLYPH)).toBe(false);
  });

  it('leaves ordinary typing alone', () => {
    expect(isFunctionKeyTextInsertion('insertText', 'a')).toBe(false);
    expect(isFunctionKeyTextInsertion('insertText', '?')).toBe(false);
    expect(isFunctionKeyTextInsertion('insertText', 'a slack adapter?')).toBe(false);
  });

  it('leaves an emoji alone (an astral codepoint, not a two-unit range hit)', () => {
    expect(isFunctionKeyTextInsertion('insertText', String.fromCodePoint(0x1f600))).toBe(false);
  });

  it('leaves text that merely contains a function-key character alone', () => {
    expect(isFunctionKeyTextInsertion('insertText', `adapter?${RIGHT_ARROW}`)).toBe(false);
  });

  it('ignores a deletion, a history step, and a paste with no data', () => {
    expect(isFunctionKeyTextInsertion('deleteContentBackward', null)).toBe(false);
    expect(isFunctionKeyTextInsertion('historyUndo', null)).toBe(false);
    expect(isFunctionKeyTextInsertion('insertFromPaste', null)).toBe(false);
  });
});

// The test env is `node`, so `document` is the stub in `src/test-setup.ts`: it
// keeps a listener list and dispatches a plain object to it. That pins the
// wiring: the event name the guard listens on, and the cancel it answers with.
describe('installNoFunctionKeyText', () => {
  beforeAll(() => {
    installNoFunctionKeyText();
  });

  function type(data: string): boolean {
    let prevented = false;
    document.dispatchEvent({
      type: 'beforeinput',
      inputType: 'insertText',
      data,
      preventDefault: () => { prevented = true; },
    } as unknown as Event);
    return prevented;
  }

  it('cancels an insertion of a function-key character', () => {
    expect(type(RIGHT_ARROW)).toBe(true);
  });

  it('lets ordinary typing through', () => {
    expect(type('a')).toBe(false);
  });
});
