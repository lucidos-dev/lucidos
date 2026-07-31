import { describe, it, expect } from 'vitest';
import { encodeRepoPath, parseRepoPath } from '../store';

describe('encodeRepoPath / parseRepoPath', () => {
  it('round-trips file mode', () => {
    const enc = encodeRepoPath('repo-1', 'file', 'src/foo.rs');
    expect(enc).toBe('repo:repo-1:file:src/foo.rs');
    expect(parseRepoPath(enc)).toEqual({ repoId: 'repo-1', mode: 'file', path: 'src/foo.rs' });
  });

  it('round-trips diff mode with changeId', () => {
    const enc = encodeRepoPath('r', 'diff', 'a/b.md', 'cid-42');
    expect(enc).toBe('repo:r:diff#cid-42:a/b.md');
    expect(parseRepoPath(enc)).toEqual({ repoId: 'r', mode: 'diff', changeId: 'cid-42', path: 'a/b.md' });
  });

  it('encodes diff without changeId as legacy diff (no #)', () => {
    expect(encodeRepoPath('r', 'diff', 'a.md')).toBe('repo:r:diff:a.md');
  });

  it('parses legacy diff encoding (no changeId) — used by entries persisted before the changeId change', () => {
    expect(parseRepoPath('repo:r:diff:a.md')).toEqual({ repoId: 'r', mode: 'diff', path: 'a.md' });
  });

  it('preserves colons inside the path segment', () => {
    const enc = encodeRepoPath('r', 'diff', 'src/weird:file.rs', 'cid');
    expect(parseRepoPath(enc)).toEqual({ repoId: 'r', mode: 'diff', changeId: 'cid', path: 'src/weird:file.rs' });
  });

  it('returns null for non-repo paths', () => {
    expect(parseRepoPath('artifacts/foo.md')).toBeNull();
    expect(parseRepoPath('')).toBeNull();
  });

  it('returns null for repo path with unknown mode segment', () => {
    expect(parseRepoPath('repo:r:weird:a.md')).toBeNull();
  });

  // `file_path` reaches this parser from outside the app (an app iframe's
  // lucidos.ui.navigate, an LLM navigate_ui), so a structurally incomplete
  // encoding must be rejected rather than yielding an empty repoId (a repo
  // selection that is neither null nor a real id) or an empty path.
  it('returns null when a segment is empty', () => {
    expect(parseRepoPath('repo::file:src/a.rs')).toBeNull();   // no repo id
    expect(parseRepoPath('repo:r1:file:')).toBeNull();         // no path
    expect(parseRepoPath('repo:r1:file')).toBeNull();          // no path segment at all
    expect(parseRepoPath('repo:r1:diff:')).toBeNull();
    expect(parseRepoPath('repo:r1:diff#:a.md')).toBeNull();    // empty change id
    expect(parseRepoPath('repo:')).toBeNull();
  });
});
