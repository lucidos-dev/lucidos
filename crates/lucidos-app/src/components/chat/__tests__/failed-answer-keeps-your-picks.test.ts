/**
 * A failed multi-select answer hands the answer back.
 *
 * Reported from an iOS PWA: a three-option answer failed to send twice. Each
 * failure cleared the toggles and the composer, so the user re-picked all three
 * from scratch before the third attempt landed. The clearing is right, being
 * the send gesture. Losing it on a failure is not.
 *
 * The transport half of the same report is fixed in
 * `src/api/client/chat-answer-retry.test.ts`. This is what the user keeps when
 * the retry does not save them either.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { recoverableAnswerDraft } from '../prompt-input-helpers';

const untouched = { currentIds: [], currentDraft: '', domText: '' };

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
    // The user re-picked while the POST was in flight. That is the newer
    // answer, so the stale one stays gone.
    expect(recoverableAnswerDraft({
      sentIds: ['opt-0'], sentText: '', currentIds: ['opt-2'], currentDraft: '', domText: '',
    }).ids).toBeNull();
  });

  it('keeps a fresh keystroke, whichever copy of the composer holds it', () => {
    // The draft copy...
    expect(recoverableAnswerDraft({
      sentIds: [], sentText: 'old', currentIds: [], currentDraft: 'new', domText: '',
    }).text).toBeNull();
    // ...and the textarea copy, which a keystroke reaches first. Restoring over
    // it would leave the two disagreeing. `resolveComposerText` prefers the
    // draft, so the stale text is what a retry would send.
    expect(recoverableAnswerDraft({
      sentIds: [], sentText: 'old', currentIds: [], currentDraft: '', domText: 'new',
    }).text).toBeNull();
  });

  it('restores with no textarea node mounted', () => {
    expect(recoverableAnswerDraft({
      sentIds: ['opt-0'], sentText: 'text', currentIds: [], currentDraft: '', domText: null,
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
    // The sync effect writes the box from the draft and resizes it. Writing
    // both would be two sources for one value.
    const body = submitMultiBody();
    expect(body).toMatch(/updateCompose\(focused, \{ text: recovered\.text \}\)/);
    expect(body).not.toMatch(/writeComposerValue\(el, recovered/);
  });

  it('the single-select card rolls back and says nothing', () => {
    expect(cardSource).not.toMatch(/showToast\('Could not send answer/);
    expect(cardSource).toMatch(/if \(!ok\) localPending\.value = null;/);
  });
});
