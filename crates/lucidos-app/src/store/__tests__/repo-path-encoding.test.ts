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

  // A git ref may contain `/` (`origin/main`, and every coding-agent branch) and
  // may contain `#`; what it may NOT contain is `:` (git check-ref-format), which
  // is exactly what makes the `:` split above safe. Slicing the qualifier at the
  // FIRST `#` is what keeps the second one part of the ref.
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

  // The encoder is the exact inverse of the parser, so a locator that survives a
  // parse re-encodes to the identical string. This is the property that keeps the
  // grammar from growing a form only one of the two halves knows about.
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

  // `file_path` reaches this parser from outside the app (an app iframe's
  // lucidos.ui.navigate, an LLM navigate_ui), so a structurally incomplete
  // encoding must be rejected rather than yielding an empty repoId (a repo
  // selection that is neither null nor a real id), an empty path, or an empty
  // ref (which would silently mean HEAD rather than the revision the caller
  // tried to name).
  it('returns null when a segment is empty', () => {
    expect(parseRepoPath('repo::file:src/a.rs')).toBeNull();   // no repo id
    expect(parseRepoPath('repo:r1:file:')).toBeNull();         // no path
    expect(parseRepoPath('repo:r1:file')).toBeNull();          // no path segment at all
    expect(parseRepoPath('repo:r1:diff:')).toBeNull();
    expect(parseRepoPath('repo:r1:diff#:a.md')).toBeNull();    // empty change id
    expect(parseRepoPath('repo:r1:file#:a.rs')).toBeNull();    // empty ref
    expect(parseRepoPath('repo:r1:file#main:')).toBeNull();    // ref but no path
    expect(parseRepoPath('repo:r1::a.rs')).toBeNull();         // no mode segment
    expect(parseRepoPath('repo:')).toBeNull();
  });
});
