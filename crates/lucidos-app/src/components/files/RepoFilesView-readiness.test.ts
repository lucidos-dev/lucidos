import { describe, it, expect } from 'vitest';
import { repoFilesContentReady } from './RepoFilesView';

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
    expect(repoFilesContentReady({ filesStatus: 'loading', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
    expect(repoFilesContentReady({ filesStatus: 'not-loaded', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
    expect(repoFilesContentReady({ filesStatus: 'failed', diffStatus: 'loaded', mode: 'changes' })).toBe(true);
  });

  it('all mode: never ready while the file tree is failed', () => {
    expect(repoFilesContentReady({ filesStatus: 'failed', diffStatus: 'loaded', mode: 'all' })).toBe(false);
  });
});
