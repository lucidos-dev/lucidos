import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { filePreviewModalBody, filePreviewModalTitle } from '../FilePreviewModal';
import { FilePreviewInline } from '../FilePreviewInline';
import { RepoFileContent } from '../RepoFilePreview';

const REPO_ID = '3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03';

/** The body is a hookless dispatcher, so it can be called as a plain function
 *  and its vnode inspected (same approach as StoreTabSkeleton.test.tsx). The
 *  child component is compared BY REFERENCE, which is the assertion that
 *  matters: the modal must render the Files panel's own previews, not a second
 *  renderer of its own. */
function body(path: string, layout: 'desktop' | 'mobile' = 'desktop') {
  return filePreviewModalBody(path, layout) as VNode<Record<string, unknown>>;
}

describe('the modal renders the Files panel previews, never its own', () => {
  it('shows a workspace data file through FilePreviewInline', () => {
    const node = body('artifacts/research/report.md');
    expect(node.type).toBe(FilePreviewInline);
    expect(node.props.path).toBe('artifacts/research/report.md');
  });

  it('forwards the active layout, so the inline preview actually mounts', () => {
    expect(body('artifacts/notes.md', 'mobile').props.layout).toBe('mobile');
  });

  it('shows a repo file through RepoFileContent, at its repo-relative path', () => {
    const node = body(`repo:${REPO_ID}:file:src/main.rs`);
    expect(node.type).toBe(RepoFileContent);
    expect(node.props.repoId).toBe(REPO_ID);
    expect(node.props.path).toBe('src/main.rs');
  });

  // The Files panel may be bound to a different repository, whose pending
  // coding-agent branch is not this file's revision.
  it('reads the repo file at HEAD, not at the bound repository branch', () => {
    expect(body(`repo:${REPO_ID}:file:src/main.rs`).props.gitRef).toBeNull();
  });

  // The locator is how a caller names a revision the panel could not have
  // guessed: a coding agent's worktree branch, a tag, a sha.
  it('reads the repo file at the ref the locator names', () => {
    const node = body(`repo:${REPO_ID}:file#claude-code/2026-08-05:src/main.rs`);
    expect(node.props.gitRef).toBe('claude-code/2026-08-05');
    expect(node.props.path).toBe('src/main.rs');
  });

  // A diff locator previews the file, and the change is which revision of it:
  // `RepoFileContent` fetches the end state through /api/v1/changes/:id/file,
  // correct for a pending branch and an applied post-merge sha alike. Reading
  // HEAD here would show the file WITHOUT the change the citation is about.
  it('reads a diff locator at its change, not at HEAD', () => {
    const node = body(`repo:${REPO_ID}:diff#change-7:src/main.rs`);
    expect(node.type).toBe(RepoFileContent);
    expect(node.props.changeId).toBe('change-7');
    expect(node.props.path).toBe('src/main.rs');
  });

  // A legacy changeId-less diff locator (persisted before the id was embedded)
  // has no change to read at, so it degrades to HEAD rather than failing.
  it('degrades a changeId-less diff locator to HEAD', () => {
    const node = body(`repo:${REPO_ID}:diff:src/main.rs`);
    expect(node.props.changeId).toBeUndefined();
    expect(node.props.gitRef).toBeNull();
  });

  it('passes no change id for a plain file locator', () => {
    expect(body(`repo:${REPO_ID}:file:src/main.rs`).props.changeId).toBeUndefined();
  });

  // A malformed encoding is not a repo path, and resolveFileTarget has already
  // turned it into a data path by the time it reaches here.
  it('treats a non-repo path as a data file', () => {
    expect(body('artifacts/repo::file:x').type).toBe(FilePreviewInline);
  });
});

describe('the modal names the file the way the citation did', () => {
  it('names a data file by its basename, with the full path beneath', () => {
    expect(filePreviewModalTitle('artifacts/research/report.md', null))
      .toEqual({ name: 'report.md', detail: 'artifacts/research/report.md' });
  });

  // The raw encoding would read as uuid soup in the header.
  it('names a repo file by its repo-relative path, not the encoding', () => {
    expect(filePreviewModalTitle(`repo:${REPO_ID}:file:src/main.rs`, null))
      .toEqual({ name: 'main.rs', detail: 'src/main.rs' });
  });

  it('appends a single cited line', () => {
    expect(filePreviewModalTitle('artifacts/notes.md', { start: 510, end: 510 }).name)
      .toBe('notes.md:510');
  });

  it('appends a cited range', () => {
    expect(filePreviewModalTitle('artifacts/notes.md', { start: 510, end: 520 }).name)
      .toBe('notes.md:510-520');
  });
});
