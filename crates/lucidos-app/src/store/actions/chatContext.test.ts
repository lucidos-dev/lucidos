import { describe, it, expect, beforeEach } from 'vitest';
import { panelOverlay, selectedLines } from '../store';
import { currentChatContext } from './chatContext';

// Both file previews now render the same line-numbered source view, so a line
// range picked in either one must reach the message the same way.
describe('currentChatContext', () => {
  const ENCODED = 'repo:repo-1:file:src/main.rs';

  beforeEach(() => {
    panelOverlay.value = null;
    selectedLines.value = null;
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
    panelOverlay.value = { type: 'app-ui', app: { id: 'habit-tracker', name: 'Habit Tracker', description: '' } };
    selectedLines.value = { start: 1, end: 2 };

    expect(currentChatContext()).toEqual({ app_context: { app_id: 'habit-tracker' } });
  });
});
