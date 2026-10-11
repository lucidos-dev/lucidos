import { describe, it, expect, beforeEach } from 'vitest';
import { panelOverlay, selectedLines, filePreviewModal, createPreviewViewState } from '../store';
import { currentChatContext } from './chatContext';

// Both file previews now render the same line-numbered source view, so a line
// range picked in either one must reach the message the same way.
describe('currentChatContext', () => {
  const ENCODED = 'repo:repo-1:file:src/main.rs';

  beforeEach(() => {
    panelOverlay.value = null;
    selectedLines.value = null;
    filePreviewModal.value = null;
  });

  // A file link in a reply opens the preview modal over the Files panel. The
  // message carries what the panel shows, never the glance over it.
  it('carries the Files panel selection, not the preview modal over it', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    selectedLines.value = { start: 3, end: 4 };
    const view = createPreviewViewState();
    view.selectedLines.value = { start: 99, end: 120 };
    filePreviewModal.value = { id: 1, path: 'artifacts/other.md', range: { start: 99, end: 120 }, view };

    expect(currentChatContext()).toEqual({
      file_context: { path: 'artifacts/notes.md', lines: [3, 4] },
    });
  });

  it('is null with nothing contextual on screen', () => {
    expect(currentChatContext()).toBeNull();
  });

  it('carries a data file with no selection', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };

    expect(currentChatContext()).toEqual({
      file_context: { path: 'artifacts/notes.md', lines: undefined },
    });
  });

  it('carries a data file with its selected line range', () => {
    panelOverlay.value = { type: 'file-preview', path: 'artifacts/notes.md' };
    selectedLines.value = { start: 10, end: 20 };

    expect(currentChatContext()).toEqual({
      file_context: { path: 'artifacts/notes.md', lines: [10, 20] },
    });
  });

  it('carries a repo file with its selected line range', () => {
    panelOverlay.value = { type: 'file-preview', path: ENCODED };
    selectedLines.value = { start: 510, end: 510 };

    expect(currentChatContext()).toEqual({
      repo_file_context: { repo_id: 'repo-1', path: 'src/main.rs', lines: [510, 510] },
    });
  });

  it('sends the repo path decoded, never the encoding', () => {
    panelOverlay.value = { type: 'file-preview', path: ENCODED };

    expect(currentChatContext()).toEqual({
      repo_file_context: { repo_id: 'repo-1', path: 'src/main.rs', lines: undefined },
    });
  });

  it('prefers an open app over any file preview', () => {
    panelOverlay.value = { type: 'app-ui', app: { id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false } };
    selectedLines.value = { start: 1, end: 2 };

    expect(currentChatContext()).toEqual({ app_context: { app_id: 'habit-tracker' } });
  });
});
