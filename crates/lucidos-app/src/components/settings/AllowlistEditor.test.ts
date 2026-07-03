import { describe, it, expect } from 'vitest';
import { parseAllowlist, serializeAllowlist } from './AllowlistEditor';

describe('parseAllowlist', () => {
  it('splits the # header from editable pattern rows', () => {
    const { header, patterns } = parseAllowlist('# a\n# b\nBash(git:*)\nPython\n');
    expect(header).toEqual(['# a', '# b']);
    expect(patterns).toEqual(['Bash(git:*)', 'Python']);
  });

  it('drops blank lines and trims patterns', () => {
    const { header, patterns } = parseAllowlist('# h\n\n  Bash(git:*)  \n\nPython\n');
    expect(header).toEqual(['# h']);
    expect(patterns).toEqual(['Bash(git:*)', 'Python']);
  });

  it('returns no patterns for a header-only file', () => {
    expect(parseAllowlist('# just a header\n').patterns).toEqual([]);
  });
});

describe('serializeAllowlist', () => {
  it('joins header + patterns with a single trailing newline', () => {
    expect(serializeAllowlist(['# h'], ['Bash(git:*)'])).toBe('# h\nBash(git:*)\n');
  });

  it('drops empty/whitespace pattern rows (an unfilled Add row never persists)', () => {
    expect(serializeAllowlist(['# h'], ['Bash(git:*)', '', '  '])).toBe('# h\nBash(git:*)\n');
  });
});

describe('round-trip', () => {
  it('normalizing a messy file is idempotent', () => {
    const p1 = parseAllowlist('# h\n\nBash(git:*)\n\n\nPython\n');
    const once = serializeAllowlist(p1.header, p1.patterns);
    const p2 = parseAllowlist(once);
    const twice = serializeAllowlist(p2.header, p2.patterns);
    expect(once).toBe('# h\nBash(git:*)\nPython\n');
    expect(twice).toBe(once);
  });

  it('an empty file normalizes to a single newline (so dirty stays false on load)', () => {
    const { header, patterns } = parseAllowlist('');
    expect(serializeAllowlist(header, patterns)).toBe('\n');
  });
});
