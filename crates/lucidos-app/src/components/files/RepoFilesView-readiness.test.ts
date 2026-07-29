import { describe, it, expect } from 'vitest';
import { repoFilesContentReady } from './RepoFilesView';

// Each mode gates only on the data it actually renders, so the view is ready as
// soon as that data lands — not blocked on the other mode's fetch:
//  - All Files draws the file tree (repoFiles) → gate on the tree.
//  - Changes draws the diff (repoDiff), not the tree → gate on the diff.
// Gating Changes on the diff alone makes the diff list appear as soon as the
// diff loads instead of waiting for the whole-repo tree listing (which only All
// Files needs, and which is often slower). The diff gate still prevents the
// original "No changes" flash — an empty [] rendered while the diff is in flight
// (before the single-file overlay opens over it).
describe('repoFilesContentReady', () => {
  it('all mode: ready once the file tree is loaded, regardless of diff', () => {
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'not-loaded', mode: 'all' })).toBe(true);
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'loading', mode: 'all' })).toBe(true);
  });

  it('all mode: not ready while the file tree is still loading', () => {
    expect(repoFilesContentReady({ filesStatus: 'loading', diffStatus: 'loaded', mode: 'all' })).toBe(false);
    expect(repoFilesContentReady({ filesStatus: 'not-loaded', diffStatus: 'loaded', mode: 'all' })).toBe(false);
  });

  it('changes mode: NOT ready while the diff is still loading (the "No changes" flash)', () => {
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'loading', mode: 'changes' })).toBe(false);
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'not-loaded', mode: 'changes' })).toBe(false);
  });

  it('changes mode: ready as soon as the diff loads, without waiting for the file tree', () => {
    expect(repoFilesContentReady({ filesStatus: 'loaded', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
    // The whole-tree listing that only All Files needs must not gate the diff list.
    expect(repoFilesContentReady({ filesStatus: 'loading', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
    expect(repoFilesContentReady({ filesStatus: 'not-loaded', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
    // A tree-listing failure doesn't block the diff (RepoFilesView surfaces it
    // only when the user switches to All Files).
    expect(repoFilesContentReady({ filesStatus: 'failed', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
  });

  it('all mode: never ready while the file tree is failed', () => {
    expect(repoFilesContentReady({ filesStatus: 'failed', diffStatus: 'loaded', mode: 'all' })).toBe(false);
  });
});
