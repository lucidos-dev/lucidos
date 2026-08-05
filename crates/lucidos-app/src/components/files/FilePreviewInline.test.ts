import { describe, it, expect } from 'vitest';
import { basename, sourceLinesFor } from './FilePreviewInline';

describe('basename', () => {
  it('returns the last segment of a nested path', () => {
    expect(basename('apps/no-role-playing-0.1.2.lucidos-plugin'))
      .toBe('no-role-playing-0.1.2.lucidos-plugin');
  });

  it('returns the path itself when it has no slashes', () => {
    expect(basename('foo.bin')).toBe('foo.bin');
  });

  it('handles deeply nested paths', () => {
    expect(basename('a/b/c/file.tar.gz')).toBe('file.tar.gz');
  });

  it('returns empty string for empty input', () => {
    expect(basename('')).toBe('');
  });

  it('returns empty string for trailing-slash paths', () => {
    expect(basename('apps/')).toBe('');
  });
});

// The data-file preview shows the same line-numbered source view the repo
// preview does, so a `navigate('file', { line })` into a workspace file has a
// numbered row to scroll to and highlight. This is the branch that decides
// whether a file has source lines at all.
describe('sourceLinesFor', () => {
  it('numbers a code file line by line', () => {
    const lines = sourceLinesFor('fn main() {\n    let x = 1;\n}', 'rs', false);
    expect(lines).toHaveLength(3);
    expect(lines[1]).toContain('let');
  });

  // Content is asserted by line count, not by text: `escapeHtml` runs through a
  // real `document` and the test environment's stub cannot reproduce it, so the
  // escaping itself belongs to `escapeHtml`, not here.
  it('numbers a file with no known language', () => {
    expect(sourceLinesFor('plain <b>text</b>\nsecond', 'txt', false)).toHaveLength(2);
  });

  // A numbered line must be the file's OWN line. Reformatting valid JSON would
  // renumber it, so a `path:42` citation and the range handed to chat context
  // would both point at code that isn't in the file on disk.
  it('numbers JSON as written, never as reformatted', () => {
    expect(sourceLinesFor('{"a":1,"b":2}', 'json', false)).toHaveLength(1);
    expect(sourceLinesFor('{\n  "a": 1\n}', 'json', false)).toHaveLength(3);
  });

  it('keeps invalid JSON as written rather than failing to render', () => {
    expect(sourceLinesFor('{not json\nat all', 'json', false)).toHaveLength(2);
  });

  it('reports no source lines for a file that renders richly', () => {
    for (const ext of ['md', 'html', 'htm', 'csv', 'slides']) {
      expect(sourceLinesFor('# Title\n\nbody', ext, false), ext).toEqual([]);
    }
  });

  it('numbers a rich format once the source view is on', () => {
    expect(sourceLinesFor('# Title\n\nbody', 'md', true)).toHaveLength(3);
    expect(sourceLinesFor('a,b\n1,2', 'csv', true)).toHaveLength(2);
  });

  it('gives an empty file exactly one line', () => {
    expect(sourceLinesFor('', 'txt', false)).toEqual(['']);
  });
});
