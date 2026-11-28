import { describe, it, expect } from 'vitest';
import { encodeRepoPath, parseRepoPath } from '../store';

describe('encodeRepoPath / parseRepoPath', () => {
  it('round-trips file mode', () => {
    const enc = encodeRepoPath({ repoId: 'repo-1', mode: 'file', path: 'src/foo.rs' });
    expect(enc).toBe('repo:repo-1:file:src/foo.rs');
    expect(parseRepoPath(enc)).toEqual({ repoId: 'repo-1', mode: 'file', path: 'src/foo.rs' });
  });

  it('round-trips file mode at a named ref', () => {
    const enc = encodeRepoPath({ repoId: 'r', mode: 'file', ref: 'v1.2.0', path: 'src/foo.rs' });
    expect(enc).toBe('repo:r:file#v1.2.0:src/foo.rs');
    expect(parseRepoPath(enc)).toEqual({ repoId: 'r', mode: 'file', ref: 'v1.2.0', path: 'src/foo.rs' });
  });

  it('round-trips diff mode with changeId', () => {
    const enc = encodeRepoPath({ repoId: 'r', mode: 'diff', changeId: 'cid-42', path: 'a/b.md' });
    expect(enc).toBe('repo:r:diff#cid-42:a/b.md');
    expect(parseRepoPath(enc)).toEqual({ repoId: 'r', mode: 'diff', changeId: 'cid-42', path: 'a/b.md' });
  });

  it('encodes diff without changeId as legacy diff (no #)', () => {
    expect(encodeRepoPath({ repoId: 'r', mode: 'diff', path: 'a.md' })).toBe('repo:r:diff:a.md');
  });

  it('parses legacy diff encoding (no changeId) — used by entries persisted before the changeId change', () => {
    expect(parseRepoPath('repo:r:diff:a.md')).toEqual({ repoId: 'r', mode: 'diff', path: 'a.md' });
  });

  it('preserves colons inside the path segment', () => {
    const enc = encodeRepoPath({ repoId: 'r', mode: 'diff', changeId: 'cid', path: 'src/weird:file.rs' });
    expect(parseRepoPath(enc)).toEqual({ repoId: 'r', mode: 'diff', changeId: 'cid', path: 'src/weird:file.rs' });
  });

  it('preserves slashes and a second # inside the ref', () => {
    expect(parseRepoPath('repo:r:file#origin/main:src/a.rs'))
      .toEqual({ repoId: 'r', mode: 'file', ref: 'origin/main', path: 'src/a.rs' });
    expect(parseRepoPath('repo:r:file#feat#123:src/a.rs'))
      .toEqual({ repoId: 'r', mode: 'file', ref: 'feat#123', path: 'src/a.rs' });
  });

  it('accepts a full sha as a ref', () => {
    const sha = '0f3c1a9b2d4e6f8a0c2e4d6b8a0c2e4d6b8a0c2e';
    expect(parseRepoPath(`repo:r:file#${sha}:src/a.rs`))
      .toEqual({ repoId: 'r', mode: 'file', ref: sha, path: 'src/a.rs' });
  });

  it('re-encodes every parseable locator to itself', () => {
    const locators = [
      'repo:r:file:src/a.rs',
      'repo:r:file#main:src/a.rs',
      'repo:r:file#origin/main:src/a.rs',
      'repo:r:diff:src/a.rs',
      'repo:r:diff#cid-42:src/a.rs',
      'repo:r:file:src/weird:file.rs',
    ];
    for (const encoded of locators) {
      const parsed = parseRepoPath(encoded);
      expect(parsed).not.toBeNull();
      expect(encodeRepoPath(parsed!)).toBe(encoded);
    }
  });

  it('returns null for non-repo paths', () => {
    expect(parseRepoPath('artifacts/foo.md')).toBeNull();
    expect(parseRepoPath('')).toBeNull();
  });

  it('returns null for repo path with unknown mode segment', () => {
    expect(parseRepoPath('repo:r:weird:a.md')).toBeNull();
    expect(parseRepoPath('repo:r:weird#main:a.md')).toBeNull();
  });

  it('returns null when a segment is empty', () => {
    expect(parseRepoPath('repo::file:src/a.rs')).toBeNull();
    expect(parseRepoPath('repo:r1:file:')).toBeNull();
    expect(parseRepoPath('repo:r1:file')).toBeNull();
    expect(parseRepoPath('repo:r1:diff:')).toBeNull();
    expect(parseRepoPath('repo:r1:diff#:a.md')).toBeNull();
    expect(parseRepoPath('repo:r1:file#:a.rs')).toBeNull();
    expect(parseRepoPath('repo:r1:file#main:')).toBeNull();
    expect(parseRepoPath('repo:r1::a.rs')).toBeNull();
    expect(parseRepoPath('repo:')).toBeNull();
  });
});
