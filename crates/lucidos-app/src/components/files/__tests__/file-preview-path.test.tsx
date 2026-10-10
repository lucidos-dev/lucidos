// @vitest-environment jsdom
/**
 * The two surfaces that show a file's PATH rather than its name, and the one
 * that deliberately still shows a name.
 *
 * The breadcrumb and the changed-files row render into the DOM. The tree row
 * is hook-free, so it is invoked as a plain function and flattened.
 */
import { describe, it, expect, vi } from 'vitest';
import { render } from 'preact';
import type { DiffFile } from '../../../store/store';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';
import { FilePreviewPath } from '../FilePreviewPath';
import { ChangesFileList } from '../RepoFilesView';
import { TreeNode } from '../FolderTree';

const { revealFolderInFiles } = vi.hoisted(() => ({ revealFolderInFiles: vi.fn() }));
vi.mock('../../../store/actions/repositories', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/repositories')>()),
  revealFolderInFiles,
}));

vi.mock('../../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
}));

function changedFile(path: string): DiffFile {
  return { path, status: 'modified', hunks: [] };
}

describe('FilePreviewPath', () => {
  function renderPath(path: string): HTMLElement {
    const host = document.createElement('div');
    render(<FilePreviewPath path={path} />, host);
    return host;
  }

  it('renders each folder as a crumb button and the file name as the emphasized end', () => {
    const host = renderPath('.claude/rules/system-knowhow.md');
    const folders = [...host.querySelectorAll('button.file-preview-path-folder')].map(b => b.textContent);
    expect(folders).toEqual(['.claude', 'rules']);
    expect(host.querySelectorAll('.file-preview-path-sep')).toHaveLength(2);
    expect(host.querySelector('.file-preview-path-name')?.textContent).toBe('system-knowhow.md');
    render(null, host);
  });

  it('hides the separators from assistive tech, so the path reads as its folders', () => {
    const host = renderPath('artifacts/examples/note.md');
    for (const sep of host.querySelectorAll('.file-preview-path-sep')) {
      expect(sep.getAttribute('aria-hidden')).toBe('true');
    }
    render(null, host);
  });

  it('opens the Files view on the folder a crumb names, with the preview locator', () => {
    const locator = 'repo:repo-1:file:src/lib/mod.rs';
    const host = renderPath(locator);
    host.querySelectorAll<HTMLButtonElement>('button.file-preview-path-folder')[1].click();
    expect(revealFolderInFiles).toHaveBeenCalledWith(locator, 'src/lib');
    render(null, host);
  });

  it('renders a repo-encoded locator as the repo-relative path, never the encoding', () => {
    const out = renderPath('repo:repo-1:diff#cid-42:system-knowhow/workspace-audit.md').textContent ?? '';
    expect(out).toContain('system-knowhow');
    expect(out).toContain('workspace-audit.md');
    expect(out).not.toContain('repo-1');
    expect(out).not.toContain('cid-42');
  });

  it('renders no crumbs for a file at the root', () => {
    const host = renderPath('README.md');
    expect(host.querySelector('.file-preview-path-crumb')).toBeNull();
    expect(host.querySelector('.file-preview-path-name')?.textContent).toBe('README.md');
    render(null, host);
  });
});

describe('the changed-files list vs the file tree', () => {
  it('gives a changed-files row the wrapping `file-path` box, since it holds a whole path', () => {
    const host = document.createElement('div');
    render(<ChangesFileList files={[changedFile('system-knowhow/workspace-audit.md')]} />, host);
    expect(host.innerHTML).toContain('<span class="file-name file-path">system-knowhow/workspace-audit.md</span>');
    render(null, host);
  });

  it('leaves a tree row on the ellipsising `file-name` box, since it holds a bare name', () => {
    // The distinction is the point: `.file-path` overrides `.file-name`'s
    // nowrap, and applying it to the tree would wrap names that already fit.
    const out = vnodeToText(
      <TreeNode
        node={{ name: '', path: '', children: {}, files: [{ name: 'workspace-audit.md', path: 'system-knowhow/workspace-audit.md' }] }}
        isExpanded={() => false}
        onToggle={() => {}}
        onFileClick={() => {}}
      />,
    );
    expect(out).toContain('<span class="file-name">workspace-audit.md</span>');
    expect(out).not.toContain('file-path');
  });
});
