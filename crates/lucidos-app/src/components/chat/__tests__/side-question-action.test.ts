import { describe, it, expect } from 'vitest';
import { sideQuestionAction, sideQuestionModeActive } from '../prompt-input-helpers';

const onSend = { hasContent: true, stopOrCancelShown: false, threadStarted: true, isCodex: false };
const onStop = { ...onSend, hasContent: false, stopOrCancelShown: true };

describe('sideQuestionAction', () => {
  it('asks the draft from Send on a started, non-Codex thread', () => {
    expect(sideQuestionAction(onSend)).toEqual({ kind: 'ask-draft' });
  });

  it('turns on side-question mode from Stop, where the box is empty', () => {
    expect(sideQuestionAction(onStop)).toEqual({ kind: 'start-mode' });
  });

  it('names a thread that has not started yet', () => {
    expect(sideQuestionAction({ ...onSend, threadStarted: false }))
      .toEqual({ kind: 'unavailable', reason: 'Side questions need a started thread.' });
  });

  it('names a Codex thread, on Stop as on Send', () => {
    const reason = "Codex threads don't take side questions.";
    expect(sideQuestionAction({ ...onSend, isCodex: true })).toEqual({ kind: 'unavailable', reason });
    expect(sideQuestionAction({ ...onStop, isCodex: true })).toEqual({ kind: 'unavailable', reason });
  });

  it('names an empty draft with no turn to stop', () => {
    expect(sideQuestionAction({ ...onSend, hasContent: false }))
      .toEqual({ kind: 'unavailable', reason: 'Type the side question first.' });
  });

  it('names the thread before the draft, since typing would not help', () => {
    expect(sideQuestionAction({ hasContent: false, stopOrCancelShown: false, threadStarted: false, isCodex: true }))
      .toEqual({ kind: 'unavailable', reason: 'Side questions need a started thread.' });
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
