import { describe, it, expect } from 'vitest';
import { sidebarStateFromDiff } from './RepoFilePreview';
import type { RepoDiff } from '../../store/store';
import type { Loadable } from '../../store/types';

describe('sidebarStateFromDiff (Loadable discipline)', () => {
  it('loading diff returns a loading state (NOT hidden)', () => {
    expect(sidebarStateFromDiff({ status: 'loading' })).toEqual({ kind: 'loading' });
  });

  it('not-loaded diff returns hidden (no UI yet — startup hasn\'t fetched)', () => {
    expect(sidebarStateFromDiff({ status: 'not-loaded' })).toEqual({ kind: 'hidden' });
  });

  it('failed diff returns a failed state with the error message', () => {
    const result = sidebarStateFromDiff({ status: 'failed', error: 'Permission denied' });
    expect(result).toEqual({ kind: 'failed', error: 'Permission denied' });
  });

  it('loaded diff with files returns a files state', () => {
    const diff: Loadable<RepoDiff> = {
      status: 'loaded',
      data: {
        files: [
          { path: 'a.ts', status: 'modified', hunks: [] },
          { path: 'b.ts', status: 'added', hunks: [] },
        ],
      },
    };
    const result = sidebarStateFromDiff(diff);
    expect(result.kind).toBe('files');
    if (result.kind === 'files') {
      expect(result.files.map(f => f.path)).toEqual(['a.ts', 'b.ts']);
    }
  });

  it('loaded diff with zero files returns hidden (nothing to switch between)', () => {
    const diff: Loadable<RepoDiff> = {
      status: 'loaded',
      data: { files: [] },
    };
    expect(sidebarStateFromDiff(diff)).toEqual({ kind: 'hidden' });
  });
});
