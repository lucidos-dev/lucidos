import { describe, it, expect } from 'vitest';
import { DialogMessage, dialogParagraphs } from '../DialogMessage';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';

describe('dialogParagraphs', () => {
  it('keeps a one-paragraph message as one paragraph', () => {
    expect(dialogParagraphs('Delete this app?')).toEqual(['Delete this app?']);
  });

  it('splits on a BLANK line only', () => {
    expect(dialogParagraphs('First.\n\nSecond.')).toEqual(['First.', 'Second.']);
  });

  it('does NOT split on a single newline, so source-wrapped copy stays one paragraph', () => {
    // Dialog copy is concatenated across source lines; a lone newline is an
    // artifact of the source width, not an authored break.
    expect(dialogParagraphs('First line\nsecond line')).toEqual(['First line\nsecond line']);
  });

  it('splits a CRLF-authored message too', () => {
    // An app can hand any string to `lucidos.ui.confirm`, CRLF included; the
    // lone \r between the two newlines must not defeat the blank-line match.
    expect(dialogParagraphs('First.\r\n\r\nSecond.')).toEqual(['First.', 'Second.']);
  });

  it('tolerates whitespace-only separator lines and trims each paragraph', () => {
    expect(dialogParagraphs('  First.  \n \t \n  Second.  ')).toEqual(['First.', 'Second.']);
  });

  it('renders one empty paragraph for an empty message, so the spacing slot survives', () => {
    expect(dialogParagraphs('')).toEqual(['']);
    expect(dialogParagraphs('\n\n')).toEqual(['']);
  });
});

describe('DialogMessage', () => {
  it('emits one .confirm-message paragraph per block', () => {
    const text = vnodeToText(DialogMessage({ message: 'First.\n\nSecond.' }));
    expect((text.match(/class="confirm-message"/g) ?? []).length).toBe(2);
    expect(text).toBe('<p class="confirm-message">First.</p><p class="confirm-message">Second.</p>');
  });

  it('emits exactly one paragraph for ordinary single-block copy', () => {
    const text = vnodeToText(DialogMessage({ message: 'Are you sure?' }));
    expect(text).toBe('<p class="confirm-message">Are you sure?</p>');
  });
});
