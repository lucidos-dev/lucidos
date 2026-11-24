import { describe, it, expect } from 'vitest';
import { previewCopyPath, previewDiskPath, previewFilePath, previewFileName, splitPreviewPath } from './previewPath';

describe('previewFilePath', () => {
  it('passes a workspace data path through unchanged', () => {
    expect(previewFilePath('artifacts/research/notes.md')).toBe('artifacts/research/notes.md');
  });

  it('unwraps a repo-encoded path to the repo-relative one', () => {
    expect(previewFilePath('repo:repo-1:file:src/transforms/x.jslt')).toBe('src/transforms/x.jslt');
  });

  it('unwraps a diff locator, whose change id must never reach a display surface', () => {
    expect(previewFilePath('repo:repo-1:diff#cid-42:system-knowhow/workspace-audit.md'))
      .toBe('system-knowhow/workspace-audit.md');
  });
});

describe('previewFileName', () => {
  it('is the base name of the path', () => {
    expect(previewFileName('.claude/rules/system-knowhow.md')).toBe('system-knowhow.md');
  });

  it('handles a repo file at the clone root, which has no slash to split on', () => {
    expect(previewFileName('repo:repo-1:file:pom.xml')).toBe('pom.xml');
  });
});

describe('splitPreviewPath', () => {
  it('splits at the last separator and KEEPS the trailing slash on the folders', () => {
    const { dir, name } = splitPreviewPath('.claude/rules/system-knowhow.md');
    expect(dir).toBe('.claude/rules/');
    expect(name).toBe('system-knowhow.md');
    expect(dir + name).toBe('.claude/rules/system-knowhow.md');
  });

  it('leaves the folders empty for a file at the root', () => {
    expect(splitPreviewPath('README.md')).toEqual({ dir: '', name: 'README.md' });
  });

  it('splits the repo-relative path, not the encoding', () => {
    expect(splitPreviewPath('repo:repo-1:file:src/main.rs')).toEqual({ dir: 'src/', name: 'main.rs' });
  });
});

describe('previewDiskPath', () => {
  const WS = '/home/user/workspaces/dev';
  const REPOS = [{ id: 'repo-1', path: '/home/user/code/example-repo' }];

  it('anchors a workspace data path under the workspace data dir', () => {
    expect(previewDiskPath('artifacts/reports/pr.html', WS, REPOS))
      .toBe('/home/user/workspaces/dev/data/artifacts/reports/pr.html');
  });

  it('anchors a repo file in its own clone, never in the workspace', () => {
    expect(previewDiskPath('repo:repo-1:file:src/main.rs', WS, REPOS))
      .toBe('/home/user/code/example-repo/src/main.rs');
  });

  it('ignores a ref or a change id on the locator', () => {
    expect(previewDiskPath('repo:repo-1:file#v2.1:src/main.rs', WS, REPOS))
      .toBe('/home/user/code/example-repo/src/main.rs');
    expect(previewDiskPath('repo:repo-1:diff#cid-42:src/main.rs', WS, REPOS))
      .toBe('/home/user/code/example-repo/src/main.rs');
  });

  it('tolerates a trailing slash on either root', () => {
    expect(previewDiskPath('artifacts/x.html', `${WS}/`, REPOS))
      .toBe('/home/user/workspaces/dev/data/artifacts/x.html');
    expect(previewDiskPath('repo:repo-1:file:main.rs', WS, [{ id: 'repo-1', path: '/code/r/' }]))
      .toBe('/code/r/main.rs');
  });

  it('has no answer while a root is missing', () => {
    expect(previewDiskPath('artifacts/x.html', '', REPOS)).toBeNull();
    expect(previewDiskPath('repo:repo-9:file:main.rs', WS, REPOS)).toBeNull();
    expect(previewDiskPath('repo:repo-1:file:main.rs', WS, [])).toBeNull();
  });

  it('has no answer for system-knowhow, which is not in the workspace', () => {
    expect(previewDiskPath('system-knowhow/glossary.md', WS, REPOS)).toBeNull();
  });
});

describe('previewCopyPath', () => {
  const WS = '/home/user/workspaces/dev';
  const REPOS = [{ id: 'repo-1', path: '/home/user/code/example-repo' }];

  it('prefers the absolute disk path, and says so', () => {
    expect(previewCopyPath('artifacts/reports/pr.html', WS, REPOS))
      .toEqual({ path: '/home/user/workspaces/dev/data/artifacts/reports/pr.html', absolute: true });
  });

  it('prefers the absolute path inside a repo checkout, and says so', () => {
    expect(previewCopyPath('repo:repo-1:file:src/main.rs', WS, REPOS))
      .toEqual({ path: '/home/user/code/example-repo/src/main.rs', absolute: true });
  });

  it('falls back to the repo-relative path when the clone is not loaded, and says so', () => {
    expect(previewCopyPath('repo:repo-9:file:src/main.rs', WS, REPOS))
      .toEqual({ path: 'src/main.rs', absolute: false });
  });

  it('falls back to the workspace-relative path when the root is missing, and says so', () => {
    expect(previewCopyPath('artifacts/x.html', '', REPOS))
      .toEqual({ path: 'artifacts/x.html', absolute: false });
  });

  it('falls back for system-knowhow, which has no file on disk', () => {
    expect(previewCopyPath('system-knowhow/glossary.md', WS, REPOS))
      .toEqual({ path: 'system-knowhow/glossary.md', absolute: false });
  });
});
