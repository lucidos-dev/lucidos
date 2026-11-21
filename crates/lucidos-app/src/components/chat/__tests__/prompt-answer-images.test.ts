/**
 * A typed answer carries its images to the agent (AnswerKind's image_hashes),
 * so the composer no longer refuses an image while a question is open.
 *
 * Static source-pattern checks, like prompt-image-tap.test.ts: mounting
 * PromptInput drags in every chat signal and store.
 */
import { describe, it, expect } from 'vitest';
import { composerEnterSends } from '../prompt-input-helpers';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');
/** The source of the morph Send/Stop button. */
const sendButtonSource = (): string => promptSource.match(/key="send-cancel-morph"[\s\S]*?<\/button>/)?.[0] ?? '';
/** The source of the answer control's lone Submit. */
const submitButtonSource = (): string =>
  promptSource.match(/answerMode === 'submit' \? \([\s\S]*?<\/button>/)?.[0] ?? '';

describe('PromptInput sends images with an answer', () => {
  it('refuses no image while a question is open', () => {
    expect(promptSource).not.toMatch(/text only/i);
    expect(promptSource).not.toMatch(/disabled=\{isAnsweringQuestion\}/);
    expect(promptSource).not.toMatch(/attachMenuOpen\.value\s*&&\s*!isAnsweringQuestion/);
  });

  it('a multi-select answer carries the attached images and empties them from the draft', () => {
    const fn = promptSource.match(/async function submitMultiAnswer\(\)[\s\S]*?\n {2}\}/)?.[0] ?? '';
    expect(fn, 'submitMultiAnswer not found').not.toBe('');
    expect(fn).toContain('getAttachedImages(focused)');
    expect(fn).toMatch(/image_hashes: imageHashes/);
    expect(fn).toContain("updateCompose(focused, { text: '', image_hashes: [] })");
    // An upload still running, or failed, would answer without its image, so
    // the answer waits or is refused.
    const gate = fn.indexOf('uploadsGate(focused)');
    expect(gate).toBeGreaterThan(-1);
    expect(gate).toBeLessThan(fn.indexOf('answerThreadQuestion('));
  });

  it('images alone can submit a multi-select answer', () => {
    expect(promptSource).toMatch(/imageCount: images\.length/);
  });

  it('the pending-question walk reuses isAnsweringQuestion instead of repeating the literal', () => {
    // One gated walk feeds both consumers: `multiSelect` picks the prompt-row
    // Submit control, and the question's presence picks the answering
    // placeholder. Walking twice per keystroke is what the gate avoids.
    expect(promptSource).toMatch(
      /const\s+rawPendingQ\s*=\s*isAnsweringQuestion\s*\?\s*findLatestPendingQuestion/,
    );
    // Both consumers hang off the ONE optimism-filtered result, so a question
    // the user just answered keeps neither Submit nor the placeholder alive.
    expect(promptSource).toMatch(
      /const\s+pendingQ\s*=\s*rawPendingQ\s*&&\s*!pendingPicks\.has\(rawPendingQ\.toolUseId\)/,
    );
    expect(promptSource).toMatch(/const\s+answeringQuestionCard\s*=\s*pendingQ\s*!==\s*null/);
    expect(promptSource).toMatch(/const\s+pendingMultiQ\s*=\s*pendingQ\?\.multiSelect/);
    const literalHits = promptSource.match(/focusedStatus\s*===\s*'waiting_for_user_answer'/g) ?? [];
    expect(literalHits.length, 'literal compare should appear only inside isAnsweringQuestion').toBe(1);
  });
});

/** The source of the waiting card's lone Cancel. */
const cancelButtonSource = (): string =>
  promptSource.match(/answerMode === 'cancel' \? \([\s\S]*?<\/button>/)?.[0] ?? '';

describe('no composer button has a hold', () => {
  // A lost iOS lift read as a held finger and turned on side-question mode
  // under a plain tap on Send. Side questions start from the toggle and the
  // shortcut, which are taps.
  it('PromptInput imports no long-press machinery', () => {
    expect(promptSource).not.toMatch(/useLongPress|makeLongPressHandlers|LONG_PRESS_DELAY_MS/);
    expect(promptSource).not.toMatch(/onContextMenu/);
  });

  it('says nothing about a hold in any tooltip', () => {
    expect(promptSource).not.toMatch(/Hold for a side question/);
  });

  it('feeds the tap gate on every button that can end the row', () => {
    expect(sendButtonSource()).toContain('{...tapGateHandlers}');
    expect(submitButtonSource()).toContain('{...tapGateHandlers}');
    expect(cancelButtonSource()).toContain('{...tapGateHandlers}');
  });

  it('leaves the multi-select Submit with no press handlers', () => {
    expect(promptSource).not.toContain('primaryPressHandlers');
  });

  it('shows a pill whose × leaves the mode and keeps the text', () => {
    expect(promptSource).toMatch(/<Disclosure key="side-question-mode" open=\{sideQuestionMode\}>/);
    expect(promptSource).toMatch(/onClick=\{\(\) => setSideQuestionMode\(false\)\}/);
  });

  it('turns the mode on and off without asking or sending', () => {
    expect(promptSource).not.toContain('submit(true)');
    expect(promptSource).toMatch(/function setSideQuestionMode\(on: boolean\): void \{[^]*?updateComposeSelection\(threadId, \{ sideQuestionMode: on \}\);/);
  });
});

describe('in side-question mode the round button asks, in every thread state', () => {
  it('takes over from a waiting card\'s Submit once the box holds text', () => {
    expect(promptSource).toMatch(/const answersCard = isAnsweringQuestion && !\(sideQuestionMode && morphHasContent\);/);
    expect(promptSource).toContain('const actionSlot = answersCard ? answerControl');
  });

  it('leaves Submit no "Ask" face of its own', () => {
    expect(submitButtonSource()).not.toMatch(/sideQuestionMode/);
  });

  it('asks whenever the mode is on', () => {
    expect(promptSource).toMatch(/\}, sideQuestionMode\);/);
  });
});

describe('the Side question shortcut', () => {
  it('says why where a side question cannot be asked', () => {
    expect(promptSource).toMatch(/if \(sideQuestionRefusal !== null\) \{\s*showToast\(sideQuestionRefusal, 'info'\);\s*return;/);
  });

  it('toggles the mode through the one function the row toggle shares', () => {
    expect(promptSource).toMatch(/function toggleSideQuestionMode\(\): void \{[^]*?setSideQuestionMode\(!sideQuestionMode\);/);
    expect(promptSource).toMatch(/promptSideQuestionRequested\.value = false;\s*toggleSideQuestionMode\(\);/);
  });

  it('draws the row toggle only where a side question can be asked, on the same toggle', () => {
    expect(promptSource).toMatch(
      /sideQuestionRefusal === null \? sideQuestionToggleAction\(sideQuestionMode, toggleSideQuestionMode\) : null/,
    );
  });

  it('puts focus in the box inside the gesture when it turns the mode on', () => {
    expect(promptSource).toMatch(/updateComposeSelection\(threadId, \{ sideQuestionMode: on \}\);\s*if \(on\) focusIfNeeded\(inputRef\.current\);/);
  });

  // The full rule and its cases live in composer-enter-key.test.ts.
  it('keeps its chord from also sending the draft', () => {
    expect(promptSource).toMatch(/if \(composerEnterSends\(e, currentComposerEnterKey\(\), isMobile\(\)\)\)/);
    const altEnter = { key: 'Enter', altKey: true, shiftKey: false, metaKey: false, ctrlKey: false, isComposing: false, keyCode: 13 };
    expect(composerEnterSends(altEnter, 'send', false)).toBe(false);
    expect(composerEnterSends(altEnter, 'newline', false)).toBe(false);
  });
});
