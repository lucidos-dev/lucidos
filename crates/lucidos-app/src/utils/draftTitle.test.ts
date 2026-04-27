import { describe, it, expect } from 'vitest';
import { draftTitle, DRAFT_TITLE_MAX, DRAFT_FALLBACK_TITLE } from './draftTitle';

describe('draftTitle', () => {
  it('returns the fallback when the text is empty', () => {
    expect(draftTitle('')).toBe(DRAFT_FALLBACK_TITLE);
  });

  it('returns the fallback when the text is only whitespace', () => {
    expect(draftTitle('   \n\t  ')).toBe(DRAFT_FALLBACK_TITLE);
  });

  it('returns the trimmed first non-empty line for a single line', () => {
    expect(draftTitle('hello world')).toBe('hello world');
  });

  it('takes the first non-empty line, ignoring leading blank lines', () => {
    expect(draftTitle('\n\n  first real line\nmore stuff')).toBe('first real line');
  });

  it('trims surrounding whitespace from the first line', () => {
    expect(draftTitle('   spaced   \nrest')).toBe('spaced');
  });

  it('truncates lines longer than the cap with an ellipsis', () => {
    const long = 'a'.repeat(DRAFT_TITLE_MAX + 20);
    const out = draftTitle(long);
    expect(out.endsWith('…')).toBe(true);
    // ellipsis + content totals DRAFT_TITLE_MAX visible chars
    expect([...out].length).toBe(DRAFT_TITLE_MAX);
  });

  it('does not truncate when the first line is exactly at the cap', () => {
    const exact = 'b'.repeat(DRAFT_TITLE_MAX);
    expect(draftTitle(exact)).toBe(exact);
  });

  it('handles multi-byte characters without slicing them in half', () => {
    // 50 emoji chars; each is multi-byte. Truncation must not produce
    // garbage glyphs.
    const flag = '🚀';
    const long = flag.repeat(DRAFT_TITLE_MAX + 5);
    const out = draftTitle(long);
    expect(out.endsWith('…')).toBe(true);
    expect([...out].length).toBe(DRAFT_TITLE_MAX);
    // every visible char (minus the ellipsis) is the same emoji
    expect([...out].slice(0, -1).every(c => c === flag)).toBe(true);
  });

  it('treats CRLF and CR line endings the same as LF', () => {
    expect(draftTitle('first\r\nsecond')).toBe('first');
    expect(draftTitle('first\rsecond')).toBe('first');
  });
});
