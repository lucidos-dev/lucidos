import { describe, it, expect } from 'vitest';
import type { VNode } from 'preact';
import { RepoFileContent, previewGitRef } from './RepoFilePreview';
import { repoFileUrl } from '../../api/client';

// `RepoFileContent` is a hookless dispatcher, so it can be called as a plain
// function and its returned vnode inspected (same approach as
// StoreTabSkeleton.test.tsx). Only the media branch carries an `ext` prop, so
// that is what tells the two leaves apart without depending on function names.
function dispatch(props: { repoId: string; path: string; changeId?: string; gitRef: string | null }) {
  const node = RepoFileContent(props) as VNode<Record<string, unknown>>;
  return node.props;
}

const REPO = 'repo-b';
const BRANCH = 'claude-code/2026-08-05';

/** The Files panel is bound to one repository at a time and reads its files at
 *  that repository's pending coding-agent branch. The preview modal renders the
 *  same content for a repository the panel is NOT bound to, so the ref has to be
 *  the caller's, never the bound repository's. */
describe('RepoFileContent takes its git ref from the caller', () => {
  it('forwards a null ref (the clone HEAD) to the text leaf', () => {
    const props = dispatch({ repoId: REPO, path: 'src/main.rs', gitRef: null });
    expect(props.gitRef).toBeNull();
    expect(props.ext).toBeUndefined(); // text branch
  });

  it('forwards a branch ref to the text leaf', () => {
    const props = dispatch({ repoId: REPO, path: 'src/main.rs', gitRef: BRANCH });
    expect(props.gitRef).toBe(BRANCH);
  });

  it('forwards the ref to the media leaf too', () => {
    const props = dispatch({ repoId: REPO, path: 'docs/diagram.png', gitRef: null });
    expect(props.ext).toBe('png'); // media branch
    expect(props.gitRef).toBeNull();
  });

  it('keeps a change id alongside the ref', () => {
    const props = dispatch({ repoId: REPO, path: 'src/main.rs', changeId: 'change-7', gitRef: BRANCH });
    expect(props.changeId).toBe('change-7');
  });

  // What the two refs actually resolve to on the wire: a null ref must reach the
  // engine with no `ref` at all, which is the clone's current HEAD.
  it('reads HEAD when the ref is null, and the branch when it is not', () => {
    // Typed as the prop is, so the `?? undefined` conversion the leaves do is
    // what runs here (a bare `null` literal would be flagged as always-nullish).
    const headRef: string | null = null;
    expect(repoFileUrl(REPO, 'src/main.rs', headRef ?? undefined)).not.toContain('ref=');
    expect(repoFileUrl(REPO, 'src/main.rs', BRANCH)).toContain(`ref=${encodeURIComponent(BRANCH)}`);
  });
});

/** Which revision a locator resolves to, per surface. The panel's default is the
 *  bound repository's pending coding-agent branch; the modal's is `HEAD`, since
 *  it may be showing a repository the panel is not bound to. A ref named in the
 *  locator overrides both, which is the whole point of the `file#<ref>` form. */
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

  // A diff carries a change id, not a ref, so it can only ever take the default.
  it('takes the surface default for a diff locator', () => {
    expect(previewGitRef({ repoId: REPO, mode: 'diff', changeId: 'change-7', path: 'src/main.rs' }, BRANCH)).toBe(BRANCH);
    expect(previewGitRef({ repoId: REPO, mode: 'diff', changeId: 'change-7', path: 'src/main.rs' }, null)).toBeNull();
  });
});
