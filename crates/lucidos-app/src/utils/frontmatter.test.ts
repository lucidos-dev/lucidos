import { describe, it, expect } from 'vitest';
import { splitFrontmatter, frontmatterLineCount } from './frontmatter';

describe('splitFrontmatter', () => {
  it('reads scalars, keeping every colon after the first in the value', () => {
    const md = '---\ntype: daily-note\nbrief_refreshed_at: 2026-09-29T22:59:20+03:00\ntitle: "Quoted: yes"\n---\n# Body\n';
    const { frontmatter, body } = splitFrontmatter(md);
    expect(frontmatter).toEqual({
      kind: 'fields',
      fields: [
        ['type', 'daily-note'],
        ['brief_refreshed_at', '2026-09-29T22:59:20+03:00'],
        ['title', 'Quoted: yes'],
      ],
    });
    expect(body).toBe('# Body\n');
  });

  it('reads flow lists and block lists as lists', () => {
    const md = "---\ntags: [alpha, 'beta']\naliases:\n  - one\n  - two\nempty:\n---\ntext";
    expect(splitFrontmatter(md).frontmatter).toEqual({
      kind: 'fields',
      fields: [['tags', ['alpha', 'beta']], ['aliases', ['one', 'two']], ['empty', '']],
    });
  });

  it('splits a flow list only on commas outside quotes', () => {
    const md = '---\nauthors: ["Doe, Jane", Alex, ]\n---\n';
    expect(splitFrontmatter(md).frontmatter).toEqual({ kind: 'fields', fields: [['authors', ['Doe, Jane', 'Alex']]] });
  });

  it('drops trailing comments but keeps a # inside quotes or a word', () => {
    const md = '---\nstatus: open # todo\ntitle: "Issue #4" # note\nref: pr#12\ntags: [a, b] # two\n---\n';
    expect(splitFrontmatter(md).frontmatter).toEqual({
      kind: 'fields',
      fields: [['status', 'open'], ['title', 'Issue #4'], ['ref', 'pr#12'], ['tags', ['a', 'b']]],
    });
  });

  it('keeps a list of maps raw instead of showing each map as a chip', () => {
    const md = '---\npeople:\n  - name: A\n---\n';
    expect(splitFrontmatter(md).frontmatter).toEqual({ kind: 'raw', text: 'people:\n  - name: A' });
  });

  it('keeps a nested flow collection raw', () => {
    expect(splitFrontmatter('---\nmatrix: [[1, 2], [3]]\n---\n').frontmatter).toEqual({
      kind: 'raw',
      text: 'matrix: [[1, 2], [3]]',
    });
  });

  it('keeps YAML beyond the simple subset raw instead of guessing', () => {
    const md = '---\nauthor:\n  name: Someone\nsummary: |\n  line\n---\nbody';
    expect(splitFrontmatter(md)).toEqual({
      frontmatter: { kind: 'raw', text: 'author:\n  name: Someone\nsummary: |\n  line' },
      body: 'body',
    });
  });

  it('leaves a document that opens with a horizontal rule alone', () => {
    for (const md of ['---\nJust a paragraph.\n---\n', '---\nno closing fence: here\n', '# Title\n---\nkey: value\n---\n']) {
      expect(splitFrontmatter(md)).toEqual({ frontmatter: null, body: md });
    }
  });

  it('accepts CRLF line endings and a `...` closing fence', () => {
    const { frontmatter, body } = splitFrontmatter('---\r\nstatus: open\r\n...\r\nbody');
    expect(frontmatter).toEqual({ kind: 'fields', fields: [['status', 'open']] });
    expect(body).toBe('body');
  });
});

describe('frontmatterLineCount', () => {
  it('agrees with splitFrontmatter on where the body starts, for LF files', () => {
    // 5 lines before the body: ---, title, todo:, "  - a", the closing ---.
    const md = '---\ntitle: Notes\ntodo:\n  - a\n---\nbody\n';
    expect(frontmatterLineCount(md)).toBe(5);
  });

  it('returns 0 for a file with no frontmatter', () => {
    expect(frontmatterLineCount('# Plain\n\nText.\n')).toBe(0);
  });

  it('detects a CRLF frontmatter block the same way splitFrontmatter does', () => {
    // Each line of a `'\n'`-split CRLF file keeps a trailing `\r`, which the
    // shared fence/key-line regexes must still match (the bug this pins: they
    // silently didn't, so a CRLF file was treated as having no frontmatter).
    const md = '---\r\ntitle: My Doc\r\ntodo:\r\n  - [ ] milk\r\n---\r\n- [ ] real task\r\n';
    expect(frontmatterLineCount(md)).toBe(5);
    expect(splitFrontmatter(md).body).toBe('- [ ] real task\n');
  });
});
