// @vitest-environment jsdom
// A relative link inside a repo markdown file must open its target through
// Lucidos' own preview. Never a default `<a>` navigation: the tester-reported
// gap was `RepoFileText` rendering `MarkdownDocument` with no `onClick` at
// all.
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  getRepoFileContent: vi.fn(() => Promise.resolve('[sibling](../guide.md)')),
  getChangeFileContent: vi.fn(() => Promise.resolve('[sibling](../guide.md)')),
}));

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getRepoFileContent: mocks.getRepoFileContent,
  getChangeFileContent: mocks.getChangeFileContent,
}));
vi.mock('../../../store/actions/artifacts', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/artifacts')>()),
  openFilePreview: mocks.openFilePreview,
}));

import { RepoFileContent } from '../RepoFilePreview';

let host: HTMLDivElement;

beforeEach(() => {
  vi.clearAllMocks();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

it('routes a relative link in a repo markdown file through the host preview, never a default navigation', async () => {
  await act(async () => {
    render(<RepoFileContent repoId="repo-1" path="docs/notes/start.md" gitRef="main" />, host);
  });
  await act(async () => {}); // let the resolved fetch's state update flush

  const link = host.querySelector<HTMLAnchorElement>('a[href="../guide.md"]')!;
  expect(link).not.toBeNull();

  const event = new MouseEvent('click', { bubbles: true, cancelable: true });
  act(() => { link.dispatchEvent(event); });

  expect(event.defaultPrevented).toBe(true);
  // Resolved against the file's OWN directory inside the SAME checkout and
  // ref, never the workspace data root.
  expect(mocks.openFilePreview).toHaveBeenCalledWith('repo:repo-1:file#main:docs/guide.md');
});

it('anchors a repo-root-relative link at the checkout root, not the workspace data root', async () => {
  mocks.getRepoFileContent.mockReturnValueOnce(Promise.resolve('[root doc](/docs/root.md)'));
  await act(async () => {
    render(<RepoFileContent repoId="repo-1" path="src/deep/nested.md" gitRef={null} />, host);
  });
  await act(async () => {});

  const link = host.querySelector<HTMLAnchorElement>('a[href="/docs/root.md"]')!;
  act(() => { link.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); });

  expect(mocks.openFilePreview).toHaveBeenCalledWith('repo:repo-1:file:docs/root.md');
});

// A sibling is a plain file, never another diff entry. A sibling outside the
// change has no diff entry to resolve to at all (Codex review finding).
it('opens a sibling link as a file, not a diff entry, while viewing a change\'s whole-file view', async () => {
  await act(async () => {
    render(
      <RepoFileContent repoId="repo-1" path="docs/notes/start.md" changeId="change-7" gitRef="agent-branch" />,
      host,
    );
  });
  await act(async () => {});

  const link = host.querySelector<HTMLAnchorElement>('a[href="../guide.md"]')!;
  act(() => { link.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); });

  expect(mocks.openFilePreview).toHaveBeenCalledWith('repo:repo-1:file#agent-branch:docs/guide.md');
});
