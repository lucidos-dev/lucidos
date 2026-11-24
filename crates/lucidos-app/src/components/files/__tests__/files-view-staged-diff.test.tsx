// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import {
  repoSource, repoPending, repoViewMode, repoDiff, repoFiles, repositories, artifacts,
} from '../../../store/store';
import { FilesView } from '../FilesView';

vi.mock('../../../store/actions/chat', () => ({ loadRepositories: vi.fn() }));

let host: HTMLDivElement;

beforeEach(() => {
  repositories.value = { status: 'loaded', data: [] };
  artifacts.value = { status: 'loaded', data: [] };
  repoSource.value = null;
  repoPending.value = null;
  repoFiles.value = { status: 'not-loaded' };
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => render(null, host));
  host.remove();
});

describe('FilesView with a staged diff', () => {
  it('stays in the repo view while the diff loads with no repo bound', () => {
    repoViewMode.value = 'changes';
    repoDiff.value = { status: 'loading' };

    act(() => render(<FilesView />, host));

    expect(host.querySelector('.workspace-files-view')).toBeNull();
  });

  it('falls back to the workspace view when a diff with no repo bound failed', () => {
    repoViewMode.value = 'changes';
    repoDiff.value = { status: 'failed', error: 'Failed to load diff: gone' };

    act(() => render(<FilesView />, host));

    expect(host.querySelector('.workspace-files-view')).not.toBeNull();
  });

  it('shows the workspace view when no repo is bound and no diff is staged', () => {
    repoViewMode.value = 'all';
    repoDiff.value = { status: 'not-loaded' };

    act(() => render(<FilesView />, host));

    expect(host.querySelector('.workspace-files-view')).not.toBeNull();
  });
});
