import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { recoverableAnswerDraft } from '../prompt-input-helpers';

const untouched = { currentIds: [], currentDraft: '' };

describe('recoverableAnswerDraft', () => {
  it('gives the toggled options back so a retry is one tap', () => {
    expect(recoverableAnswerDraft({
      sentIds: ['opt-0', 'opt-1', 'opt-3'], sentText: '', ...untouched,
    })).toEqual({ ids: ['opt-0', 'opt-1', 'opt-3'], text: null });
  });

  it('gives the typed custom answer back too', () => {
    expect(recoverableAnswerDraft({
      sentIds: [], sentText: 'also do the watchdog', ...untouched,
    })).toEqual({ ids: null, text: 'also do the watchdog' });
  });

  it('keeps a fresh pick made during the failure window', () => {
    expect(recoverableAnswerDraft({
      sentIds: ['opt-0'], sentText: '', currentIds: ['opt-2'], currentDraft: '',
    }).ids).toBeNull();
  });

  it('keeps a fresh keystroke', () => {
    expect(recoverableAnswerDraft({
      sentIds: [], sentText: 'old', currentIds: [], currentDraft: 'new',
    }).text).toBeNull();
  });

  it('restores into an empty composer', () => {
    expect(recoverableAnswerDraft({
      sentIds: ['opt-0'], sentText: 'text', currentIds: [], currentDraft: '',
    })).toEqual({ ids: ['opt-0'], text: 'text' });
  });

  it('writes nothing back when the answer was empty', () => {
    expect(recoverableAnswerDraft({ sentIds: [], sentText: '', ...untouched }))
      .toEqual({ ids: null, text: null });
  });
});

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource: string = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');
const cardSource: string = readFileSync(resolve(here, '../QuestionCard.tsx'), 'utf-8');

function submitMultiBody(): string {
  const fn = promptSource.match(/async function submitMultiAnswer\(\)[\s\S]*?\n {2}\}/);
  expect(fn, 'submitMultiAnswer() not found').not.toBeNull();
  return fn![0];
}

describe('the submit sites leave the message to the action', () => {
  it('the multi-select submit recovers instead of toasting', () => {
    const body = submitMultiBody();
    expect(body).toContain('recoverableAnswerDraft(');
    expect(body).not.toMatch(/showToast\('Could not send answer/);
  });

  it('the multi-select submit restores through the draft, not the textarea', () => {
    const body = submitMultiBody();
    expect(body).toMatch(/updateCompose\(focused, \{ text: recovered\.text \}\)/);
    expect(body).not.toMatch(/writeComposerValue\(el, recovered/);
  });

  it('the single-select card rolls back and says nothing', () => {
    expect(cardSource).not.toMatch(/showToast\('Could not send answer/);
    expect(cardSource).toMatch(/else pendingAnswers\.clear\(toolUseId\);/);
  });
});
