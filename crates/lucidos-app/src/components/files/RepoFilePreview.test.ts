import { describe, it, expect } from 'vitest';
import { shouldRenderMarkdownDiff, shouldShowWholeFile } from './RepoFilePreview';

describe('shouldRenderMarkdownDiff', () => {
  it('renders for an internal Lucidos change (changeId present)', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: 'change-1', branchRef: null, filePreviewSourceOn: false,
    })).toBe(true);
  });

  it('renders for an external-repo Claude Code session that has only a branch ref', () => {
    // viewThreadCcDiff() leaves repoSelectedChangeId null because external-repo
    // Claude Code sessions never produce a Lucidos `Change` row — they only have the
    // worktree branch in repoPending. Before the fix, this returned false and
    // the .md diff fell back to the unified DiffView instead of RenderedDiff.
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: null, branchRef: 'claude-code/2026-04-29', filePreviewSourceOn: false,
    })).toBe(true);
  });

  it('does not render when neither changeId nor branch ref is available', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: null, branchRef: null, filePreviewSourceOn: false,
    })).toBe(false);
  });

  it('does not render for non-md files', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'ts', fileStatus: 'modified', activeChangeId: 'change-1', branchRef: null, filePreviewSourceOn: false,
    })).toBe(false);
  });

  it('does not render for deleted files (no after-content to fetch)', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'deleted', activeChangeId: 'change-1', branchRef: null, filePreviewSourceOn: false,
    })).toBe(false);
  });

  it('does not render when filePreviewSource is on (user opted into raw diff)', () => {
    expect(shouldRenderMarkdownDiff({
      ext: 'md', fileStatus: 'modified', activeChangeId: 'change-1', branchRef: null, filePreviewSourceOn: true,
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
