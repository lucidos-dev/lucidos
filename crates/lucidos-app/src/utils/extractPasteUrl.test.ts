import { describe, it, expect } from 'vitest';
import { extractPasteUrl, escapeMarkdownLinkText } from './extractPasteUrl';

describe('extractPasteUrl', () => {
  it('extracts plain https URL', () => {
    expect(extractPasteUrl('https://vg.no/article')).toBe('https://vg.no/article');
  });

  it('extracts plain http URL', () => {
    expect(extractPasteUrl('http://example.com')).toBe('http://example.com');
  });

  it('extracts thread ref URL', () => {
    expect(extractPasteUrl('thread:dev/abc-123-uuid')).toBe('thread:dev/abc-123-uuid');
  });

  it('extracts mailto URL', () => {
    expect(extractPasteUrl('mailto:foo@example.com')).toBe('mailto:foo@example.com');
  });

  it('extracts ftp and file URLs', () => {
    expect(extractPasteUrl('ftp://files.example.com/x')).toBe('ftp://files.example.com/x');
    expect(extractPasteUrl('file:///Users/me/doc.txt')).toBe('file:///Users/me/doc.txt');
  });

  it('trims surrounding whitespace from plain URL', () => {
    expect(extractPasteUrl('  https://vg.no  \n')).toBe('https://vg.no');
  });

  it('extracts URL from markdown link', () => {
    expect(extractPasteUrl('[Some Title](https://vg.no/article)')).toBe('https://vg.no/article');
  });

  it('extracts URL from markdown link with thread ref', () => {
    expect(extractPasteUrl('[Thread Title](thread:dev/abc-123-uuid)')).toBe('thread:dev/abc-123-uuid');
  });

  it('trims surrounding whitespace from markdown link', () => {
    expect(extractPasteUrl('  [Title](https://vg.no)  ')).toBe('https://vg.no');
  });

  it('returns null for plain text without URL', () => {
    expect(extractPasteUrl('hello world')).toBeNull();
    expect(extractPasteUrl('')).toBeNull();
  });

  it('returns null for text containing a URL but not exclusively', () => {
    expect(extractPasteUrl('check this https://vg.no out')).toBeNull();
    expect(extractPasteUrl('Visit [the site](https://vg.no) today')).toBeNull();
  });

  it('returns null for things that look url-like but are not allowed schemes', () => {
    expect(extractPasteUrl('localhost:3000')).toBeNull();
    expect(extractPasteUrl('Tue: meeting at 3pm')).toBeNull();
    expect(extractPasteUrl('javascript:alert(1)')).toBeNull();
  });

  it('returns null for markdown link with disallowed scheme', () => {
    expect(extractPasteUrl('[bad](javascript:alert(1))')).toBeNull();
  });

  // A crafted clipboard payload must not be able to break out of the
  // [title](url) substitution by closing the link early and opening another.
  it('returns null for plain URL containing markdown delimiters', () => {
    expect(extractPasteUrl('https://evil.com/)[innocent](https://real.com')).toBeNull();
    expect(extractPasteUrl('https://x.com)hi')).toBeNull();
    expect(extractPasteUrl('https://x.com]hi')).toBeNull();
  });

  it('returns null for markdown link whose URL contains markdown delimiters', () => {
    expect(extractPasteUrl('[t](https://evil.com/)[hi](https://real.com)')).toBeNull();
  });
});

describe('escapeMarkdownLinkText', () => {
  it('escapes ] so it cannot close the link title early', () => {
    expect(escapeMarkdownLinkText('array[0]')).toBe('array[0\\]');
  });

  it('escapes backslash to prevent unintended escape sequences', () => {
    expect(escapeMarkdownLinkText('a\\b')).toBe('a\\\\b');
  });

  it('leaves ordinary text untouched', () => {
    expect(escapeMarkdownLinkText('hello world')).toBe('hello world');
  });
});
