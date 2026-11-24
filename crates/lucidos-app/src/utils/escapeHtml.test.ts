// @vitest-environment jsdom
import { describe, it, expect, beforeAll } from 'vitest';
import { escapeHtml, stripHtml } from './escapeHtml';

const parsedInto: Document[] = [];

beforeAll(() => {
  const real = document.implementation.createHTMLDocument.bind(document.implementation);
  document.implementation.createHTMLDocument = ((title?: string) => {
    const doc = real(title ?? '');
    parsedInto.push(doc);
    return doc;
  }) as typeof document.implementation.createHTMLDocument;
});

describe('stripHtml', () => {
  it('parses into a document with no browsing context', () => {
    stripHtml('<b>first call</b>');
    expect(parsedInto.length).toBeGreaterThan(0);
    for (const doc of parsedInto) {
      expect(doc).not.toBe(document);
      expect(doc.defaultView).toBeNull();
    }
  });

  it('keeps untrusted markup out of the live document', () => {
    const before = document.body.innerHTML;
    stripHtml('<img src=x onerror="globalThis.__pwned = true">');
    stripHtml('<iframe src="//evil.example/"></iframe>');
    stripHtml('<script>globalThis.__pwned = true</script>');

    expect(document.querySelector('img')).toBeNull();
    expect(document.querySelector('iframe')).toBeNull();
    expect(document.querySelector('script')).toBeNull();
    expect(document.body.innerHTML).toBe(before);
    expect((globalThis as Record<string, unknown>).__pwned).toBeUndefined();
  });

  it('returns the fragment text', () => {
    expect(stripHtml('<b>hi</b> there')).toBe('hi there');
    expect(stripHtml('plain')).toBe('plain');
    expect(stripHtml('')).toBe('');
    expect(stripHtml('a &amp; b')).toBe('a & b');
  });

  it('drops the attribute that carried the payload', () => {
    expect(stripHtml('<img src=x onerror="fetch(1)">')).toBe('');
    expect(stripHtml('<span onclick="x()">label</span>')).toBe('label');
  });
});

describe('escapeHtml', () => {
  it('escapes the three characters that open a tag or an entity', () => {
    expect(escapeHtml('<b>')).toBe('&lt;b&gt;');
    expect(escapeHtml('a & b')).toBe('a &amp; b');
    expect(escapeHtml('')).toBe('');
  });

  it('shares no mutable element with stripHtml', () => {
    stripHtml('<p>left behind</p>');
    expect(escapeHtml('a<b')).toBe('a&lt;b');
    escapeHtml('<script>');
    expect(stripHtml('<i>x</i>')).toBe('x');
    expect(escapeHtml('&')).toBe('&amp;');
  });
});
