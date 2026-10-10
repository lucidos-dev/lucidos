import { describe, it, expect, beforeEach } from 'vitest';
import { diffBodyKind, sideBySideDiffAvailable, shouldRenderMarkdownDiff, shouldShowWholeFile } from '../diffBody';
import {
  diffFitsSideBySide,
  diffWholeFile,
  encodeRepoPath,
  filePreviewSource,
  panelOverlay,
  repoDiff,
  repoPending,
  repoSelectedChangeId,
} from '../store';
import type { DiffFile, RepoLocator } from '../store';

describe('shouldRenderMarkdownDiff', () => {
  it('renders for an internal Lucidos change (changeId present)', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: 'change-1', gitRef: null, filePreviewSourceOn: false,
    })).toBe(true);
  });

  it('renders for an external-repo Claude Code session that has only a branch ref', () => {
    // viewThreadCcDiff() leaves repoSelectedChangeId null because external-repo
    // Claude Code sessions never produce a Lucidos `Change` row: they only have
    // the worktree branch in repoPending. Before the fix, this returned false and
    // the .md diff fell back to the unified DiffView instead of RenderedDiff.
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: null, gitRef: 'claude-code/2026-04-29', filePreviewSourceOn: false,
    })).toBe(true);
  });

  it('does not render when neither changeId nor branch ref is available', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: null, gitRef: null, filePreviewSourceOn: false,
    })).toBe(false);
  });

  it('does not render for non-md files', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'ts', fileStatus: 'modified', activeChangeId: 'change-1', gitRef: null, filePreviewSourceOn: false,
    })).toBe(false);
  });

  it('does not render for deleted files (no after-content to fetch)', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'deleted', activeChangeId: 'change-1', gitRef: null, filePreviewSourceOn: false,
    })).toBe(false);
  });

  it('does not render when filePreviewSource is on (user opted into raw diff)', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: 'change-1', gitRef: null, filePreviewSourceOn: true,
    })).toBe(false);
  });
});

describe('shouldShowWholeFile', () => {
  it('shows the whole file when the toggle is on for a modified file', () => {
    expect(shouldShowWholeFile({ wholeFileOn: true, fileStatus: 'modified' })).toBe(true);
  });

  it('shows the whole file for an added file', () => {
    expect(shouldShowWholeFile({ wholeFileOn: true, fileStatus: 'added' })).toBe(true);
  });

  it('stays on the diff when the toggle is off', () => {
    expect(shouldShowWholeFile({ wholeFileOn: false, fileStatus: 'modified' })).toBe(false);
  });

  it('suppresses the whole-file view for a deleted file (no end state)', () => {
    expect(shouldShowWholeFile({ wholeFileOn: true, fileStatus: 'deleted' })).toBe(false);
  });
});

/** Which body the diff preview shows. Derived once so the header (which offers
 *  "Side by side" only for the raw hunks) and the body cannot disagree about
 *  what is on screen. */
describe('diffBodyKind', () => {
  function preview(file: DiffFile, changeId = 'change-1'): void {
    repoDiff.value = { status: 'loaded', data: { files: [file] } };
    const locator: RepoLocator = { repoId: 'repo-1', mode: 'diff', changeId, path: file.path };
    panelOverlay.value = { type: 'file-preview', path: encodeRepoPath(locator) };
  }

  const modified = (path: string): DiffFile => ({ path, status: 'modified', hunks: [] });

  beforeEach(() => {
    panelOverlay.value = null;
    repoDiff.value = { status: 'not-loaded' };
    repoPending.value = null;
    repoSelectedChangeId.value = null;
    diffWholeFile.value = null;
    filePreviewSource.value = false;
  });

  it('is the hunks for a modified source file', () => {
    preview(modified('src/main.rs'));
    expect(diffBodyKind.value).toBe('hunks');
  });

  it('is the whole file when the toggle is on', () => {
    preview(modified('src/main.rs'));
    diffWholeFile.value = true;
    expect(diffBodyKind.value).toBe('whole-file');
  });

  it('is the whole file by default for an added file (its diff is all additions)', () => {
    preview({ path: 'src/new.rs', status: 'added', hunks: [] });
    expect(diffBodyKind.value).toBe('whole-file');
  });

  it('has no end state for a deleted file with the toggle on', () => {
    preview({ path: 'src/gone.rs', status: 'deleted', hunks: [] });
    diffWholeFile.value = true;
    expect(diffBodyKind.value).toBe('no-end-state');
  });

  it('is the rendered markdown for an .md file with a change to fetch', () => {
    preview(modified('docs/guide.md'));
    expect(diffBodyKind.value).toBe('rendered-markdown');
  });

  // The Source toggle is how a reader asks for the raw diff of a markdown file,
  // and that IS the hunks, so side by side becomes available there.
  it('falls back to the hunks for markdown when Source is on', () => {
    preview(modified('docs/guide.md'));
    filePreviewSource.value = true;
    expect(diffBodyKind.value).toBe('hunks');
  });

  // An external-repo coding-agent session produces no Change row, so the ref is
  // all there is to fetch the post-change body with.
  it('renders markdown off the pending branch when there is no change id', () => {
    repoDiff.value = { status: 'loaded', data: { files: [modified('docs/guide.md')] } };
    panelOverlay.value = {
      type: 'file-preview',
      path: encodeRepoPath({ repoId: 'repo-1', mode: 'diff', path: 'docs/guide.md' }),
    };
    expect(diffBodyKind.value).toBe('hunks');
    repoPending.value = { branch_name: 'claude-code/2026-08-05' } as never;
    expect(diffBodyKind.value).toBe('rendered-markdown');
  });
});

/** Null means "not showing a diff", which is what the header keys off to hide
 *  every diff-only control. Each of these is a distinct way to get there. */
describe('diffBodyKind is null when there is no diff on screen', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    repoDiff.value = { status: 'not-loaded' };
    diffWholeFile.value = null;
    filePreviewSource.value = false;
  });

  it('with no preview open', () => {
    expect(diffBodyKind.value).toBeNull();
  });

  it('for a data-file preview', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    expect(diffBodyKind.value).toBeNull();
  });

  it('for a repo FILE locator', () => {
    panelOverlay.value = {
      type: 'file-preview',
      path: encodeRepoPath({ repoId: 'repo-1', mode: 'file', path: 'src/main.rs' }),
    };
    expect(diffBodyKind.value).toBeNull();
  });

  it('while the diff is still loading', () => {
    panelOverlay.value = {
      type: 'file-preview',
      path: encodeRepoPath({ repoId: 'repo-1', mode: 'diff', changeId: 'c', path: 'src/main.rs' }),
    };
    repoDiff.value = { status: 'loading' };
    expect(diffBodyKind.value).toBeNull();
  });

  it('when the previewed path is not in the loaded diff', () => {
    repoDiff.value = { status: 'loaded', data: { files: [{ path: 'other.rs', status: 'modified', hunks: [] }] } };
    panelOverlay.value = {
      type: 'file-preview',
      path: encodeRepoPath({ repoId: 'repo-1', mode: 'diff', changeId: 'c', path: 'src/main.rs' }),
    };
    expect(diffBodyKind.value).toBeNull();
  });
});

/** What the header keys the "Side by side" toggle off. Both halves matter: the
 *  toggle must not appear over a body it would do nothing to, nor on a surface
 *  with no room for two columns. */
describe('sideBySideDiffAvailable', () => {
  beforeEach(() => {
    panelOverlay.value = {
      type: 'file-preview',
      path: encodeRepoPath({ repoId: 'repo-1', mode: 'diff', changeId: 'c', path: 'src/main.rs' }),
    };
    repoDiff.value = { status: 'loaded', data: { files: [{ path: 'src/main.rs', status: 'modified', hunks: [] }] } };
    repoPending.value = null;
    repoSelectedChangeId.value = null;
    diffWholeFile.value = null;
    filePreviewSource.value = false;
    diffFitsSideBySide.value = true;
  });

  it('is offered over the hunks with room for two columns', () => {
    expect(sideBySideDiffAvailable.value).toBe(true);
  });

  it('is withheld when there is no room', () => {
    diffFitsSideBySide.value = false;
    expect(sideBySideDiffAvailable.value).toBe(false);
  });

  it('is withheld over the whole-file view', () => {
    diffWholeFile.value = true;
    expect(sideBySideDiffAvailable.value).toBe(false);
  });

  it('is withheld over a rendered markdown diff', () => {
    repoDiff.value = { status: 'loaded', data: { files: [{ path: 'docs/guide.md', status: 'modified', hunks: [] }] } };
    panelOverlay.value = {
      type: 'file-preview',
      path: encodeRepoPath({ repoId: 'repo-1', mode: 'diff', changeId: 'c', path: 'docs/guide.md' }),
    };
    expect(sideBySideDiffAvailable.value).toBe(false);
  });

  it('is withheld when no diff is on screen at all', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    expect(sideBySideDiffAvailable.value).toBe(false);
  });
});
