import { describe, it, expect } from 'vitest';
import { sideQuestionBlocker, sideQuestionModeActive } from '../prompt-input-helpers';

const started = { threadStarted: true };

describe('sideQuestionBlocker', () => {
  it('lets any started thread turn on the mode, whatever the box holds', () => {
    expect(sideQuestionBlocker(started)).toBeNull();
  });

  it('names a thread that has not started yet', () => {
    expect(sideQuestionBlocker({ ...started, threadStarted: false }))
      .toBe('Side questions need a started thread.');
  });

});

describe('sideQuestionModeActive', () => {
  const on = { stored: true, threadStarted: true };

  it('is in force when stored on a started thread', () => {
    expect(sideQuestionModeActive(on)).toBe(true);
    expect(sideQuestionModeActive({ ...on, stored: false })).toBe(false);
  });

  it('is off in a draft, whatever is stored', () => {
    expect(sideQuestionModeActive({ ...on, threadStarted: false })).toBe(false);
  });
});
