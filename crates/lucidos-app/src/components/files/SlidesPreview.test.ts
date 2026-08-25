import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';

/** A `.slides` deck is an artifact the model writes, and several of its fields
 *  are injected as HTML on the host origin. A site that skips `slideHtml` runs
 *  an `<img onerror>` from a deck, so the scan is over the source rather than
 *  over one rendered node: a new node type added later is covered too. */
describe('SlidesPreview: every HTML injection is scrubbed', () => {
  const source: string = readFileSync(new URL('./SlidesPreview.tsx', import.meta.url), 'utf8');

  const sites = source.match(/dangerouslySetInnerHTML=\{\{\s*__html:[\s\S]*?\}\}/g) ?? [];
  const attributes = source.match(/dangerouslySetInnerHTML\s*=/g) ?? [];

  // The scan below only judges the sites it managed to parse, so a site it
  // cannot parse would pass silently. Counting the bare attribute name catches
  // that: a reformatted or nested site leaves the two counts apart.
  it('parses every injection site', () => {
    expect(sites.length).toBeGreaterThan(0);
    expect(sites.length).toBe(attributes.length);
  });

  it('routes each one through slideHtml', () => {
    const unscrubbed = sites.filter((s) => !s.includes('__html: slideHtml('));
    expect(unscrubbed).toEqual([]);
  });

  it('scrubs with the markdown sanitizer, not a local copy', () => {
    expect(source).toContain("import { sanitizeHtmlFragments } from '../../utils/renderMarkdown'");
    expect(source).toContain('return sanitizeHtmlFragments(raw || \'\');');
  });
});
