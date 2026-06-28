import { describe, it, expect } from 'vitest';
import { repoFilesContentReady } from './RepoFilesView';

// Regression: the LoadingFade skeleton refactor gated the Files view on repoFiles
// (the tree) alone, but Changes mode renders the diff (repoDiff). repoFiles
// usually resolves first, so the view flipped to "loaded" while repoDiff was
// still in flight and ChangesFileList rendered an empty [] → a spurious
// "No changes" flashed before the real diff (and the single-file overlay)
// arrived. Changes-mode readiness must therefore gate on the diff load too.
describe('repoFilesContentReady', () => {
  it('all mode: ready once the file tree is loaded, regardless of diff', () => {
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'not-loaded', mode: 'all' })).toBe(true);
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'loading', mode: 'all' })).toBe(true);
  });

  it('all mode: not ready while the file tree is still loading', () => {
    expect(repoFilesContentReady({ filesStatus: 'loading', diffStatus: 'loaded', mode: 'all' })).toBe(false);
    expect(repoFilesContentReady({ filesStatus: 'not-loaded', diffStatus: 'loaded', mode: 'all' })).toBe(false);
  });

  it('changes mode: NOT ready when the diff is still loading even if the tree is loaded (the "No changes" flash)', () => {
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'loading', mode: 'changes' })).toBe(false);
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'not-loaded', mode: 'changes' })).toBe(false);
  });

  it('changes mode: ready only once BOTH the tree and the diff are loaded', () => {
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
  });

  it('never ready while the file tree is failed/loading in either mode', () => {
    expect(repoFilesContentReady({ filesStatus: 'failed', diffStatus: 'loaded', mode: 'changes' })).toBe(false);
    expect(repoFilesContentReady({ filesStatus: 'failed', diffStatus: 'loaded', mode: 'all' })).toBe(false);
  });
});
