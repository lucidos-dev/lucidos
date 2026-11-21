import { describe, it, expect, beforeEach, vi } from 'vitest';
import { panelOverlay, repoSource, repoSelectedChangeId, selectedLines, repoDiff, repoPending } from '../store';

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

const { openRepoFilePreview, openEncodedRepoFilePreview } = await import('./repositories');

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

// The app-iframe / NavigationRequested entry point: the path arrives ALREADY
// encoded and the caller has no repo context, so the action binds one.
describe('openEncodedRepoFilePreview (navigate bridge)', () => {
  const ENCODED = 'repo:repo-2:file:src/main/resources/transforms/x.jslt';

  beforeEach(() => {
    pushNavState.mockClear();
    replaceNavState.mockClear();
    revealContentPane.mockClear();
    panelOverlay.value = null;
    repoSource.value = null;
    repoSelectedChangeId.value = null;
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    selectedLines.value = null;
    localStorage.clear();
  });

  it('declines a workspace data path so the caller falls back to the /data preview', () => {
    expect(openEncodedRepoFilePreview('artifacts/notes.md')).toBe(false);
    expect(panelOverlay.value).toBeNull();
    expect(repoSource.value).toBeNull();
  });

  it('declines a malformed repo: path', () => {
    expect(openEncodedRepoFilePreview('artifacts/repo:r1:weird:a.md')).toBe(false);
    expect(panelOverlay.value).toBeNull();
  });

  it('opens the encoded path and binds the repo from a cold start', () => {
    expect(openEncodedRepoFilePreview(ENCODED)).toBe(true);

    expect(panelOverlay.value).toEqual({ type: 'file-preview', path: ENCODED });
    expect(repoSource.value).toBe('repo-2');
    expect(revealContentPane).toHaveBeenCalledTimes(1);
    expect(pushNavState).toHaveBeenCalledTimes(1);
  });

  it('rebinds off another repo and drops its diff, so the sidebar cannot mix repos', () => {
    repoSource.value = 'repo-1';
    repoSelectedChangeId.value = 'change-7';
    repoDiff.value = { status: 'loaded', data: { files: [{ path: 'other.rs', status: 'modified', hunks: [] }] } };
    repoPending.value = { branch_name: 'b', files: ['other.rs'], description: 'd', thread_id: null };

    openEncodedRepoFilePreview(ENCODED);

    expect(repoSource.value).toBe('repo-2');
    expect(repoDiff.value.status).toBe('not-loaded');
    expect(repoPending.value).toBeNull();
    expect(repoSelectedChangeId.value).toBeNull();
  });

  it('keeps the change selection when already on that repo', () => {
    repoSource.value = 'repo-2';
    repoSelectedChangeId.value = 'change-7';

    openEncodedRepoFilePreview(ENCODED);

    expect(repoSource.value).toBe('repo-2');
    expect(repoSelectedChangeId.value).toBe('change-7');
    expect(panelOverlay.value).toEqual({ type: 'file-preview', path: ENCODED });
  });

  it('clears selectedLines so a prior file\'s highlighted range does not leak', () => {
    repoSource.value = 'repo-2';
    selectedLines.value = { start: 5, end: 10 };

    openEncodedRepoFilePreview(ENCODED);

    expect(selectedLines.value).toBeNull();
  });
});
