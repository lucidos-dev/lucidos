/** The source, wrap and fullscreen shortcuts act only where the content
 *  header offers the button.
 *
 *  They share the header's own gate, so editing a file, or a pane showing
 *  something else, leaves the keystroke doing nothing.
 *
 *  Plan: `docs/plans/2026-10-01-shortcuts-for-every-toggle.md`. */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../../store/actions/apps', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../../store/actions/apps')>();
  return { ...actual, toggleAppFullscreen: vi.fn() };
});

import { toggleSourceView, toggleLineWrap, toggleAppFullscreenIfShown } from '../ContentHeaderActions';
import { toggleAppFullscreen } from '../../../store/actions/apps';
import { panelOverlay, filePreviewSource, filePreviewWrap, filePreviewEditing } from '../../../store/store';

beforeEach(() => {
  vi.clearAllMocks();
  filePreviewSource.value = false;
  filePreviewWrap.value = false;
  filePreviewEditing.value = false;
});

describe('the source shortcut', () => {
  it('flips a rendered document to its source', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    toggleSourceView();
    expect(filePreviewSource.value).toBe(true);
  });

  it('does nothing for a file with no rendered view', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/script.py' };
    toggleSourceView();
    expect(filePreviewSource.value).toBe(false);
  });

  it('does nothing while the file is being edited', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    filePreviewEditing.value = true;
    toggleSourceView();
    expect(filePreviewSource.value).toBe(false);
  });

  it('does nothing when the pane shows no file', () => {
    panelOverlay.value = null;
    toggleSourceView();
    expect(filePreviewSource.value).toBe(false);
  });
});

describe('the wrap shortcut', () => {
  it('wraps the line-numbered source view', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/script.py' };
    toggleLineWrap();
    expect(filePreviewWrap.value).toBe(true);
  });

  it('does nothing over a rendered document, which wraps by itself', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    toggleLineWrap();
    expect(filePreviewWrap.value).toBe(false);
  });
});

describe('the fullscreen shortcut', () => {
  it('toggles fullscreen over an open app', () => {
    panelOverlay.value = { type: 'app-ui', app: { id: 'habit-tracker' } } as unknown as typeof panelOverlay.value;
    toggleAppFullscreenIfShown();
    expect(toggleAppFullscreen).toHaveBeenCalledTimes(1);
  });

  it('does nothing when the pane shows no app', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    toggleAppFullscreenIfShown();
    expect(toggleAppFullscreen).not.toHaveBeenCalled();
  });
});
