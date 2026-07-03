import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, repoSource, repoSelectedChangeId, selectedLines } from '../store';

// Spy on nav helpers so we can verify push-vs-replace without touching the
// real localStorage-backed nav stack.
const pushNavState = vi.fn();
const replaceNavState = vi.fn();
vi.mock('./navigation', () => ({ pushNavState, replaceNavState }));

// revealContentPane mutates splitRatio + writes to localStorage on desktop;
// stub it so tests don't leak that state across files.
const revealContentPane = vi.fn();
vi.mock('./pane', () => ({ revealContentPane }));

vi.mock('../../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../api/client')>();
  return {
    ...actual,
    listRepoFiles: vi.fn(),
    getChangeDiff: vi.fn(),
    getChangeById: vi.fn(),
    getRepoChanges: vi.fn(),
    getThreadCcDiff: vi.fn(),
  };
});

const { openRepoFilePreview } = await import('./repositories');

describe('openRepoFilePreview push-vs-replace', () => {
  beforeEach(() => {
    pushNavState.mockClear();
    replaceNavState.mockClear();
    revealContentPane.mockClear();
    panelOverlay.value = null;
    repoSource.value = 'repo-1';
    repoSelectedChangeId.value = null;
    selectedLines.value = null;
  });

  it('first open from outside the panel pushes a new nav entry', () => {
    openRepoFilePreview('src/main.rs', 'file');

    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(replaceNavState).not.toHaveBeenCalled();
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(panelOverlay.value?.type).toBe('file-preview');
  });

  it('switching files inside the open panel replaces, never pushes', () => {
    openRepoFilePreview('src/a.rs', 'file');
    openRepoFilePreview('src/b.rs', 'file');
    openRepoFilePreview('src/c.rs', 'file');

    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(replaceNavState).toHaveBeenCalledTimes(2);
    expect(panelOverlay.value?.type === 'file-preview' && panelOverlay.value.path).toContain('src/c.rs');
  });

  it('opening the panel from a non-file overlay pushes once, then sidebar clicks replace', () => {
    panelOverlay.value = { type: 'app-ui', app: { id: 'demo' } as any };

    openRepoFilePreview('src/a.rs', 'file');
    openRepoFilePreview('src/b.rs', 'file');

    expect(pushNavState).toHaveBeenCalledTimes(1);
    expect(replaceNavState).toHaveBeenCalledTimes(1);
  });

  it('clears selectedLines on every open so cross-file line ranges do not leak', () => {
    selectedLines.value = { start: 5, end: 10 };

    openRepoFilePreview('src/a.rs', 'file');

    expect(selectedLines.value).toBeNull();
  });

  it('no-op when no repo is selected', () => {
    repoSource.value = null;

    openRepoFilePreview('src/a.rs', 'file');

    expect(pushNavState).not.toHaveBeenCalled();
    expect(replaceNavState).not.toHaveBeenCalled();
    expect(panelOverlay.value).toBeNull();
  });

  it('diff mode threads the selected change ID into the encoded path', () => {
    repoSelectedChangeId.value = 'change-7';

    openRepoFilePreview('src/a.rs', 'diff');

    expect(panelOverlay.value).toEqual({
      type: 'file-preview',
      path: 'repo:repo-1:diff#change-7:src/a.rs',
    });
  });

  it('file mode never threads a change ID even if one is selected', () => {
    repoSelectedChangeId.value = 'change-7';

    openRepoFilePreview('src/a.rs', 'file');

    expect(panelOverlay.value).toEqual({
      type: 'file-preview',
      path: 'repo:repo-1:file:src/a.rs',
    });
  });
});
