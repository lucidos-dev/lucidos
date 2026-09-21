import { describe, it, expect } from 'vitest';
import { previewDiskPath, previewFilePath, previewFileName, splitPreviewPath } from './previewPath';

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
    // The two halves must concatenate back into the path exactly, so a caller
    // rendering them as adjacent spans never reintroduces the separator itself.
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

// What the packaged desktop client hands to the OS opener. A wrong answer here
// is an "item not found" dialog, or worse, the wrong file opened silently.
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

  // The locator's qualifier names a revision, which the file on disk does not
  // have. It must not leak into the path.
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

  // Each of these would otherwise build a path to a file that is not there.
  it('has no answer while a root is missing', () => {
    expect(previewDiskPath('artifacts/x.html', '', REPOS)).toBeNull();
    expect(previewDiskPath('repo:repo-9:file:main.rs', WS, REPOS)).toBeNull();
    expect(previewDiskPath('repo:repo-1:file:main.rs', WS, [])).toBeNull();
  });

  // system-knowhow ships inside the engine and is served from there, so there
  // is no such file under the workspace to open.
  it('has no answer for system-knowhow, which is not in the workspace', () => {
    expect(previewDiskPath('system-knowhow/glossary.md', WS, REPOS)).toBeNull();
  });
});
