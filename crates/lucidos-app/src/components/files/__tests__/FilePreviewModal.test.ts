import { beforeAll, describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { loadFilePreviewModal, filePreviewModalBody, filePreviewModalTitle } from '../FilePreviewModal';
import { FilePreviewInline } from '../FilePreviewInline';
import { RepoFileContent } from '../RepoFilePreview';

const REPO_ID = '3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03';

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

  it('reads the repo file at HEAD, not at the bound repository branch', () => {
    expect(body(`repo:${REPO_ID}:file:src/main.rs`).props.gitRef).toBeNull();
  });

  it('reads the repo file at the ref the locator names', () => {
    const node = body(`repo:${REPO_ID}:file#claude-code/2026-08-05:src/main.rs`);
    expect(node.props.gitRef).toBe('claude-code/2026-08-05');
    expect(node.props.path).toBe('src/main.rs');
  });

  it('reads a diff locator at its change, not at HEAD', () => {
    const node = body(`repo:${REPO_ID}:diff#change-7:src/main.rs`);
    expect(node.type).toBe(RepoFileContent);
    expect(node.props.changeId).toBe('change-7');
    expect(node.props.path).toBe('src/main.rs');
  });

  it('degrades a changeId-less diff locator to HEAD', () => {
    const node = body(`repo:${REPO_ID}:diff:src/main.rs`);
    expect(node.props.changeId).toBeUndefined();
    expect(node.props.gitRef).toBeNull();
  });

  it('passes no change id for a plain file locator', () => {
    expect(body(`repo:${REPO_ID}:file:src/main.rs`).props.changeId).toBeUndefined();
  });

  it('treats a non-repo path as a data file', () => {
    expect(body('artifacts/repo::file:x').type).toBe(FilePreviewInline);
  });
});

describe('the modal names the file the way the citation did', () => {
  it('names a data file by its basename, with the full path beneath', () => {
    expect(filePreviewModalTitle('artifacts/research/report.md', null))
      .toEqual({ name: 'report.md', detail: 'artifacts/research/report.md' });
  });

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

beforeAll(async () => { await loadFilePreviewModal(); });
