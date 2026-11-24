import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { SlidesPreview } from './SlidesPreview';

function rootClass(content: string): string {
  return (SlidesPreview({ content }).props as { class: string }).class;
}

describe('SlidesPreview: a malformed deck says so instead of throwing', () => {
  it.each([
    ['null', 'null'],
    ['slides as an object', '{"title":"t","slides":{}}'],
    ['a null slide', '{"title":"t","slides":[null]}'],
    ['content as a string', '{"title":"t","slides":[{"title":"s","content":"x"}]}'],
    ['list items as a string', '{"title":"t","slides":[{"title":"s","content":[{"type":"list","items":"x"}]}]}'],
  ])('%s', (_, deck) => {
    expect(rootClass(deck)).toBe('empty-state error-text');
  });

  it('still renders a well-formed deck', () => {
    expect(rootClass('{"title":"t","slides":[{"title":"s","content":[{"type":"icon","emoji":"x"}]}]}')).toBe('slides-pv');
  });
});

describe('SlidesPreview: every HTML injection is scrubbed', () => {
  const source: string = readFileSync(new URL('./SlidesPreview.tsx', import.meta.url), 'utf8');

  const sites = source.match(/dangerouslySetInnerHTML=\{\{\s*__html:[\s\S]*?\}\}/g) ?? [];
  const attributes = source.match(/dangerouslySetInnerHTML\s*=/g) ?? [];

  it('parses every injection site', () => {
    expect(sites.length).toBeGreaterThan(0);
    expect(sites.length).toBe(attributes.length);
  });

  it('routes each one through slideHtml', () => {
    const unscrubbed = sites.filter((s) => !s.includes('__html: slideHtml('));
    expect(unscrubbed).toEqual([]);
  });

  it('scrubs with the markdown sanitizer, not a local copy', () => {
    expect(source).toContain("import { sanitizeHtmlFragments } from '../../utils/markdownEngine'");
    expect(source).toContain('return sanitizeHtmlFragments(raw || \'\');');
  });
});
