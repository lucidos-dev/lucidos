import { describe, it, expect } from 'vitest';
import { externalWebUrl } from './webLink';

describe('externalWebUrl', () => {
  it.each([
    ['https://example.com/a', 'https://example.com/a'],
    ['http://example.com', 'http://example.com/'],
    ['HTTPS://example.com/a', 'https://example.com/a'],
    ['Http://example.com', 'http://example.com/'],
    ['  https://example.com ', 'https://example.com/'],
  ])('keeps the web link %s', (href, expected) => {
    expect(externalWebUrl(href, 'https:')).toBe(expected);
  });

  it('gives a protocol-relative href the page protocol', () => {
    expect(externalWebUrl('//example.com/p', 'https:')).toBe('https://example.com/p');
    expect(externalWebUrl('//example.com/p', 'http:')).toBe('http://example.com/p');
  });

  it('returns a backslash href absolute, since the browser opens it on that host', () => {
    expect(externalWebUrl('\\\\example.com/x', 'https:')).toBe('https://example.com/x');
    expect(externalWebUrl('/\\example.com', 'https:')).toBe('https://example.com/');
  });

  it('falls back to https when the page is not on the web', () => {
    expect(externalWebUrl('//example.com/p', 'file:')).toBe('https://example.com/p');
  });

  it.each(['/x', 'x/y', '#frag', '?q=1', '', 'mailto:a@b.c', 'tel:123', 'javascript:alert(1)', 'data:text/html,x', 'file:///tmp/x', 'app:thing'])(
    'ignores %j',
    (href) => {
      expect(externalWebUrl(href, 'https:')).toBeNull();
    },
  );
});
