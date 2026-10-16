import { describe, it, expect } from 'vitest';
import { mergeAllowlist, parseAllowlist, serializeAllowlist } from './AllowlistEditor';

describe('mergeAllowlist', () => {
  const base = ['Bash(git:*)', 'Python', 'Bash(ls:*)'];

  it('keeps a pattern that appeared in the file since the draft started', () => {
    expect(mergeAllowlist(base, base, [...base, 'Bash(git status)']))
      .toEqual([...base, 'Bash(git status)']);
  });

  it('keeps a pattern the draft deleted deleted, even though the file still has it', () => {
    expect(mergeAllowlist(base, ['Bash(git:*)', 'Bash(ls:*)'], base)).toEqual(['Bash(git:*)', 'Bash(ls:*)']);
  });

  it('applies the draft\'s adds and deletes on top of a grant', () => {
    expect(mergeAllowlist(base, ['Bash(git:*)', 'Bash(ls:*)', 'Bash(pwd)'], [...base, 'Bash(git status)']))
      .toEqual(['Bash(git:*)', 'Bash(ls:*)', 'Bash(pwd)', 'Bash(git status)']);
  });

  it('drops an untouched pattern the file dropped', () => {
    expect(mergeAllowlist(base, base, ['Bash(git:*)', 'Bash(ls:*)'])).toEqual(['Bash(git:*)', 'Bash(ls:*)']);
  });

  it('writes a pattern both sides added once', () => {
    expect(mergeAllowlist(base, [...base, 'Bash(pwd)'], [...base, 'Bash(pwd)'])).toEqual([...base, 'Bash(pwd)']);
  });

  it('keeps the draft\'s raw rows, empty and untrimmed ones included', () => {
    expect(mergeAllowlist(base, [...base, '  Bash(pwd) ', ''], base)).toEqual([...base, '  Bash(pwd) ', '']);
  });
});

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
