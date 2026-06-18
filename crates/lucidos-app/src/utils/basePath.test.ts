import { describe, it, expect } from 'vitest';
import { normalizeBasePath } from './basePath';

// `normalizeBasePath` turns the server-stamped `<base href>` into a path prefix
// (no trailing slash, '' at root). It is slug-agnostic — any workspace name
// works, and `/~/` marks the picker context (ADR 0014).

describe('normalizeBasePath', () => {
  it('returns the /<slug> prefix for a workspace base href', () => {
    expect(normalizeBasePath('/dev/')).toBe('/dev');
    expect(normalizeBasePath('/work-2/')).toBe('/work-2');
    expect(normalizeBasePath('/a1b2/')).toBe('/a1b2');
  });

  it('returns /~ for the picker context', () => {
    expect(normalizeBasePath('/~/')).toBe('/~');
  });

  it('returns empty at a legacy root', () => {
    expect(normalizeBasePath('/')).toBe('');
  });

  it('strips an absolute-URL base href down to its pathname', () => {
    expect(normalizeBasePath('https://host.ts.net/dev/')).toBe('/dev');
  });

  it('tolerates a missing leading slash and extra trailing slashes', () => {
    expect(normalizeBasePath('dev/')).toBe('/dev');
    expect(normalizeBasePath('/dev///')).toBe('/dev');
  });
});
