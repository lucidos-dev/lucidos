// @vitest-environment jsdom
/** File previews and diffs draw a placeholder in the shape of what will
 *  replace them, past the delay gate: numbered source lines for code, lines of
 *  text for a rendered document, and diff cards for a diff. Never a spinner. */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, type VNode } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getChangeFileContent: () => new Promise(() => {}),
  getRepoFileContent: () => new Promise(() => {}),
}));

import { FilePreviewInline } from '../FilePreviewInline';
import { RepoFileContent } from '../RepoFilePreview';
import { RenderedDiff } from '../RenderedDiff';
import { ChangesFileListSkeleton, RepoFilesView } from '../RepoFilesView';
import { filePreviewSource, repoDiff, repoSource } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

function show(node: VNode) {
  act(() => { render(node, host); });
}

function passGate() {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  vi.stubGlobal('fetch', () => new Promise(() => {}));
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  filePreviewSource.value = false;
  repoDiff.value = { status: 'not-loaded' };
  repoSource.value = null;
});

const skeleton = (sel: string) => host.querySelector(`.loading-fade-skeleton ${sel}`);

describe('workspace file preview', () => {
  it('draws nothing before the gate, then numbered source lines for code', () => {
    show(<FilePreviewInline path="scripts/tool.py" layout="desktop" />);
    expect(host.querySelector('.sk-bar')).toBeNull();
    passGate();
    expect(skeleton('.code-line .line-number')?.textContent).toBe('1');
    expect(skeleton('.code-line .line-content .sk-bar')).not.toBeNull();
    expect(host.querySelector('.loading-spinner')).toBeNull();
  });

  it('draws lines of text for a rendered document', () => {
    show(<FilePreviewInline path="notes/plan.md" layout="desktop" />);
    passGate();
    expect(skeleton('.markdown-content .sk-bar')).not.toBeNull();
    expect(skeleton('.code-line')).toBeNull();
  });
});

describe('repo previews', () => {
  it('draws numbered source lines for a repo code file', () => {
    show(<RepoFileContent repoId="r1" path="src/main.rs" gitRef={null} />);
    passGate();
    expect(skeleton('.repo-file-content .code-line .sk-bar')).not.toBeNull();
  });

  it('draws lines of text inside the rendered diff box', () => {
    show(<RenderedDiff file={{ path: 'README.md', status: 'modified', hunks: [] }} changeId="c1" repoId="r1" gitRef={null} />);
    passGate();
    expect(skeleton('.rendered-diff .markdown-content .sk-bar')).not.toBeNull();
  });

  it('draws diff cards while an app coding-agent diff loads', () => {
    repoDiff.value = { status: 'loading' };
    show(<RepoFilesView />);
    passGate();
    expect(host.querySelectorAll('.loading-fade-skeleton .diff-view')).toHaveLength(2);
    expect(host.querySelector('.loading-spinner')).toBeNull();
  });

  it('draws changed-file rows for the sidebar', () => {
    show(<ChangesFileListSkeleton />);
    expect(host.querySelectorAll('.folder-tree .repo-changed-file')).toHaveLength(6);
    expect(host.querySelector('.repo-changed-file .file-path .sk-bar')).not.toBeNull();
  });
});
