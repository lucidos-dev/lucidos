import { describe, it, expect } from 'vitest';
import { previewFilePath, previewFileName, splitPreviewPath } from './previewPath';

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
