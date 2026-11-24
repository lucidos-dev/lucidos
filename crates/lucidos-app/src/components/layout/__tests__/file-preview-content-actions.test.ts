import { describe, it, expect, beforeEach, vi } from 'vitest';

const platform = vi.hoisted(() => ({ isTauri: false, isIOSPwa: false, clipboardCopy: true }));
vi.mock('../../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/platform')>()),
  isTauri: () => platform.isTauri,
  isIOSPwa: () => platform.isIOSPwa,
  clipboardAbilities: () => ({ copy: platform.clipboardCopy, paste: false }),
}));

const { filePreviewContentActions } = await import('../ContentHeaderActions');
const { repositories, workspacePath, panelOverlay, filePreviewSource, filePreviewEditing } =
  await import('../../../store/store');

const keysOf = (path: string, mobile = false) => filePreviewContentActions(path, mobile).map((a) => a.key);

describe('filePreviewContentActions', () => {
  beforeEach(() => {
    (globalThis as unknown as { window: { location: unknown } }).window.location =
      { pathname: '/dev/', search: '', href: 'https://localhost:5251/dev/' };
    platform.isTauri = false;
    platform.isIOSPwa = false;
    platform.clipboardCopy = true;
    workspacePath.value = '/home/user/workspaces/dev';
    repositories.value = { status: 'loaded', data: [] };
    panelOverlay.value = null;
    filePreviewSource.value = false;
    filePreviewEditing.value = false;
  });

  it('returns nothing while the file is being edited', () => {
    filePreviewEditing.value = true;
    expect(filePreviewContentActions('artifacts/notes.md', false)).toEqual([]);
  });

  it('offers popout, copy-path, find, the source toggle, wrap and edit, in that order', () => {
    const path = 'artifacts/notes.md';
    panelOverlay.value = { type: 'file-preview', path };
    filePreviewSource.value = true;

    expect(keysOf(path)).toEqual(['open-in-tab', 'copy-path', 'find', 'source-toggle', 'wrap-toggle', 'edit']);
  });

  it('drops popout where the platform offers none, without breaking the rest', () => {
    platform.isIOSPwa = true;
    const path = 'artifacts/notes.md';
    panelOverlay.value = { type: 'file-preview', path };
    filePreviewSource.value = true;

    expect(keysOf(path)).toEqual(['copy-path', 'find', 'source-toggle', 'wrap-toggle', 'edit']);
  });

  it('drops copy-path where there is no clipboard, without breaking the rest', () => {
    platform.clipboardCopy = false;
    const path = 'artifacts/notes.md';
    panelOverlay.value = { type: 'file-preview', path };
    filePreviewSource.value = true;

    expect(keysOf(path)).toEqual(['open-in-tab', 'find', 'source-toggle', 'wrap-toggle', 'edit']);
  });

  it('offers neither the source toggle nor edit for a non-renderable, non-editable file', () => {
    expect(keysOf('artifacts/photo.png')).toEqual(['open-in-tab', 'copy-path']);
  });

  it('puts a pinned, disabled refresh first for a mobile diff, ahead of everything else', () => {
    const path = 'repo:repo-1:diff#cid-1:src/main.rs';
    repositories.value = { status: 'loaded', data: [{ id: 'repo-1', name: 'r', path: '/code/r' }] };
    panelOverlay.value = { type: 'file-preview', path };

    const mobileKeys = keysOf(path, true);
    expect(mobileKeys[0]).toBe('refresh');
    expect(mobileKeys).not.toContain('edit');
  });

  it('carries no mobile refresh on desktop, for the same diff', () => {
    const path = 'repo:repo-1:diff#cid-1:src/main.rs';
    repositories.value = { status: 'loaded', data: [{ id: 'repo-1', name: 'r', path: '/code/r' }] };
    panelOverlay.value = { type: 'file-preview', path };

    expect(keysOf(path, false)).not.toContain('refresh');
  });
});
