import { describe, it, expect } from 'vitest';
import { sideQuestionBlocker, sideQuestionModeActive } from '../prompt-input-helpers';

const started = { threadStarted: true, isCodex: false };

describe('sideQuestionBlocker', () => {
  it('lets any started, non-Codex thread turn on the mode, whatever the box holds', () => {
    expect(sideQuestionBlocker(started)).toBeNull();
  });

  it('names a thread that has not started yet', () => {
    expect(sideQuestionBlocker({ ...started, threadStarted: false }))
      .toBe('Side questions need a started thread.');
  });

  it('names a Codex thread', () => {
    expect(sideQuestionBlocker({ ...started, isCodex: true }))
      .toBe("Codex threads don't take side questions.");
  });

  it('names the unstarted thread first, since that is what the user can change', () => {
    expect(sideQuestionBlocker({ threadStarted: false, isCodex: true }))
      .toBe('Side questions need a started thread.');
  });
});

describe('sideQuestionModeActive', () => {
  const on = { stored: true, threadStarted: true, isCodex: false };

  it('is in force when stored on a started, non-Codex thread', () => {
    expect(sideQuestionModeActive(on)).toBe(true);
    expect(sideQuestionModeActive({ ...on, stored: false })).toBe(false);
  });

  it('is off in a draft or a Codex thread, whatever is stored', () => {
    expect(sideQuestionModeActive({ ...on, threadStarted: false })).toBe(false);
    expect(sideQuestionModeActive({ ...on, isCodex: true })).toBe(false);
  });
});
