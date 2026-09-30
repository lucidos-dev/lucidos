import { describe, it, expect } from 'vitest';
import { sideQuestionAction } from '../prompt-input-helpers';

const onSend = { hasContent: true, stopShown: false, threadStarted: true, isCodex: false };
const onStop = { ...onSend, hasContent: false, stopShown: true };

describe('sideQuestionAction', () => {
  it('asks the draft from Send on a started, non-Codex thread', () => {
    expect(sideQuestionAction(onSend)).toEqual({ kind: 'ask-draft' });
  });

  it('starts a side question draft from Stop, where the box is empty', () => {
    expect(sideQuestionAction(onStop)).toEqual({ kind: 'start-draft' });
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
    expect(sideQuestionAction({ hasContent: false, stopShown: false, threadStarted: false, isCodex: true }))
      .toEqual({ kind: 'unavailable', reason: 'Side questions need a started thread.' });
  });
});
