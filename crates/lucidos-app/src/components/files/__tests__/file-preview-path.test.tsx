/**
 * The two surfaces that show a file's PATH rather than its name, and the one
 * that deliberately still shows a name.
 *
 * Components are invoked as plain functions and the returned vnode tree is
 * flattened (the repo idiom, no DOM render library), so only hook-free
 * components can be passed in, which all three of these are.
 */
import { describe, it, expect, vi } from 'vitest';
import type { DiffFile } from '../../../store/store';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';
import { FilePreviewPath } from '../FilePreviewPath';
import { ChangesFileList } from '../RepoFilesView';
import { TreeNode } from '../FolderTree';

vi.mock('../../../api/client', () => ({
  listAppsApi: vi.fn().mockResolvedValue([]),
  getNotifications: vi.fn().mockResolvedValue({ notifications: [], unread_count: 0, has_more: false }),
  listCredentials: vi.fn().mockResolvedValue({ credentials: [] }),
}));

function changedFile(path: string): DiffFile {
  return { path, status: 'modified', hunks: [] };
}

describe('FilePreviewPath', () => {
  it('renders the whole path, with the file name as its own emphasized half', () => {
    const out = vnodeToText(<FilePreviewPath path=".claude/rules/system-knowhow.md" />);
    expect(out).toContain('<span class="file-preview-path-dir">.claude/rules/</span>');
    expect(out).toContain('<span class="file-preview-path-name">system-knowhow.md</span>');
  });

  it('renders a repo-encoded locator as the repo-relative path, never the encoding', () => {
    const out = vnodeToText(<FilePreviewPath path="repo:repo-1:diff#cid-42:system-knowhow/workspace-audit.md" />);
    expect(out).toContain('system-knowhow/');
    expect(out).toContain('workspace-audit.md');
    expect(out).not.toContain('repo-1');
    expect(out).not.toContain('cid-42');
  });

  it('drops the folders span entirely for a file at the root', () => {
    const out = vnodeToText(<FilePreviewPath path="README.md" />);
    expect(out).not.toContain('file-preview-path-dir');
    expect(out).toContain('<span class="file-preview-path-name">README.md</span>');
  });
});

describe('the changed-files list vs the file tree', () => {
  it('gives a changed-files row the wrapping `file-path` box, since it holds a whole path', () => {
    const out = vnodeToText(
      <ChangesFileList files={[changedFile('system-knowhow/workspace-audit.md')]} />,
    );
    expect(out).toContain('<span class="file-name file-path">system-knowhow/workspace-audit.md</span>');
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
