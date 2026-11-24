import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { repoFileContentLeaf, previewGitRef } from './RepoFilePreview';
import { repoFileUrl } from '../../api/client';

function dispatch(props: { repoId: string; path: string; changeId?: string; gitRef: string | null }) {
  const node = repoFileContentLeaf(props, false) as VNode<Record<string, unknown>>;
  return node.props;
}

const REPO = 'repo-b';
const BRANCH = 'claude-code/2026-08-05';

describe('RepoFileContent takes its git ref from the caller', () => {
  it('forwards a null ref (the clone HEAD) to the text leaf', () => {
    const props = dispatch({ repoId: REPO, path: 'src/main.rs', gitRef: null });
    expect(props.gitRef).toBeNull();
    expect(props.body).toBe('source');
    expect(props.kind).toBeUndefined();
  });

  it('forwards a branch ref to the text leaf', () => {
    const props = dispatch({ repoId: REPO, path: 'src/main.rs', gitRef: BRANCH });
    expect(props.gitRef).toBe(BRANCH);
  });

  it('forwards the ref to the media leaf too', () => {
    const props = dispatch({ repoId: REPO, path: 'docs/diagram.png', gitRef: null });
    expect(props.kind).toBe('image');
    expect(props.gitRef).toBeNull();
  });

  it('keeps a change id alongside the ref', () => {
    const props = dispatch({ repoId: REPO, path: 'src/main.rs', changeId: 'change-7', gitRef: BRANCH });
    expect(props.changeId).toBe('change-7');
  });

  it('reads HEAD when the ref is null, and the branch when it is not', () => {
    const headRef: string | null = null;
    expect(repoFileUrl(REPO, 'src/main.rs', headRef ?? undefined)).not.toContain('ref=');
    expect(repoFileUrl(REPO, 'src/main.rs', BRANCH)).toContain(`ref=${encodeURIComponent(BRANCH)}`);
  });
});

describe('previewGitRef: the locator names the revision, the surface names the default', () => {
  const TAG = 'v1.2.0';

  it('prefers the locator ref over the panel branch', () => {
    expect(previewGitRef({ repoId: REPO, mode: 'file', ref: TAG, path: 'src/main.rs' }, BRANCH)).toBe(TAG);
  });

  it('prefers the locator ref over the modal default of HEAD', () => {
    expect(previewGitRef({ repoId: REPO, mode: 'file', ref: TAG, path: 'src/main.rs' }, null)).toBe(TAG);
  });

  it('falls back to the panel branch when the locator names no ref', () => {
    expect(previewGitRef({ repoId: REPO, mode: 'file', path: 'src/main.rs' }, BRANCH)).toBe(BRANCH);
  });

  it('falls back to HEAD when neither names a revision', () => {
    expect(previewGitRef({ repoId: REPO, mode: 'file', path: 'src/main.rs' }, null)).toBeNull();
  });

  it('takes the surface default for a diff locator', () => {
    expect(previewGitRef({ repoId: REPO, mode: 'diff', changeId: 'change-7', path: 'src/main.rs' }, BRANCH)).toBe(BRANCH);
    expect(previewGitRef({ repoId: REPO, mode: 'diff', changeId: 'change-7', path: 'src/main.rs' }, null)).toBeNull();
  });
});
