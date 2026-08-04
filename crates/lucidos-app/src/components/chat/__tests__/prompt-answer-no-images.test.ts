/**
 * Custom UserQuestion answers are text only by design — the AnswerKind
 * payload (FreeText / MultiSelected) has no image_hashes field, and
 * chat/process.rs silently reroutes any typed text on a thread with a
 * pending question into the answer flow, dropping any attached images.
 *
 * PromptInput must:
 *   1. Refuse paste-image when answering, with a toast.
 *   2. Refuse file-picker selection when answering, with a toast.
 *   3. Refuse submit when images are already attached + question is pending.
 *   4. Disable the attach buttons + hide the dropdown when answering.
 *   5. Force-close the attach menu signal when the question arrives mid-open
 *      (otherwise the menu pops back the moment the question resolves).
 *
 * Static source-pattern checks mirror the convention used by
 * prompt-image-tap.test.ts — the alternative (mounting PromptInput) drags in
 * every chat signal/store, which other PromptInput tests deliberately avoid.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

describe('PromptInput refuses image attach while answering a UserQuestion', () => {
  it('derives isAnsweringQuestion from focusedStatus === waiting_for_user_answer', () => {
    expect(promptSource).toMatch(
      /const\s+isAnsweringQuestion\s*=\s*focusedStatus\s*===\s*'waiting_for_user_answer'/,
    );
  });

  it('handlePaste guards image clipboard items with isAnsweringQuestion + toast + early return', () => {
    const pasteFn = promptSource.match(/function handlePaste\([\s\S]*?\n  \}/);
    expect(pasteFn, 'handlePaste not found').not.toBeNull();
    const body = pasteFn![0];
    // The guard must run AFTER preventDefault (so the bytes don't leak into
    // the textarea) but BEFORE addImageFile.
    expect(body).toMatch(/e\.preventDefault\(\)[\s\S]*?if\s*\(\s*isAnsweringQuestion\s*\)[\s\S]*?showToast\(ANSWER_NO_IMAGES_TOAST[\s\S]*?return/);
    const guardIdx = body.search(/if\s*\(\s*isAnsweringQuestion\s*\)/);
    const addIdx = body.indexOf('addImageFile');
    expect(guardIdx).toBeGreaterThan(-1);
    expect(addIdx).toBeGreaterThan(guardIdx);
  });

  it('handleFileSelect guards selected files with isAnsweringQuestion + clears input + toast', () => {
    const fn = promptSource.match(/function handleFileSelect\([\s\S]*?\n  \}/);
    expect(fn, 'handleFileSelect not found').not.toBeNull();
    const body = fn![0];
    expect(body).toMatch(/if\s*\(\s*isAnsweringQuestion\s*\)\s*\{[\s\S]*?input\.value\s*=\s*''[\s\S]*?showToast\(ANSWER_NO_IMAGES_TOAST[\s\S]*?return/);
  });

  it('submit() refuses to send when images are attached and answering', () => {
    const fn = promptSource.match(/async function submit\(\)[\s\S]*?\n  \}/);
    expect(fn, 'submit() not found').not.toBeNull();
    const body = fn![0];
    expect(body).toMatch(/if\s*\(\s*isAnsweringQuestion\s*&&\s*\(currentImages\.length\s*>\s*0\s*\|\|\s*pendingForThread\.length\s*>\s*0\)\s*\)/);
    // Guard returns BEFORE queueing or sending so the silently-dropped payload
    // never reaches sendFollowup / sendMessage / sendCompose.
    const guardIdx = body.search(/isAnsweringQuestion\s*&&\s*\(currentImages\.length/);
    const queueIdx = body.indexOf('queueUploadSend');
    const beginSendIdx = body.indexOf('beginSend');
    expect(guardIdx).toBeGreaterThan(-1);
    expect(queueIdx).toBeGreaterThan(guardIdx);
    expect(beginSendIdx).toBeGreaterThan(guardIdx);
  });

  it('both attach buttons (narrow + wide) bind disabled to isAnsweringQuestion', () => {
    const attachBtns = promptSource.match(/<button[\s\S]*?aria-label="Attach image"[\s\S]*?\/?>/g) ?? [];
    expect(attachBtns.length, 'expected narrow + wide attach buttons').toBe(2);
    for (const tag of attachBtns) {
      expect(tag).toMatch(/disabled=\{isAnsweringQuestion\}/);
    }
  });

  it('attach-menu dropdown render is gated by !isAnsweringQuestion', () => {
    expect(promptSource).toMatch(/attachMenuOpen\.value\s*&&\s*!isAnsweringQuestion/);
  });

  it('force-closes attachMenuOpen via useEffect when isAnsweringQuestion flips true', () => {
    expect(promptSource).toMatch(
      /useEffect\(\(\)\s*=>\s*\{[\s\S]*?isAnsweringQuestion\s*&&\s*attachMenuOpen\.value[\s\S]*?attachMenuOpen\.value\s*=\s*false[\s\S]*?\}\s*,\s*\[\s*isAnsweringQuestion\s*\]\)/,
    );
  });

  it('the pending-question walk reuses isAnsweringQuestion instead of repeating the literal', () => {
    // One gated walk feeds both consumers: `multiSelect` picks the prompt-row
    // Submit control, and the question's mere presence picks the answering
    // placeholder. Walking twice per keystroke is what the gate exists to
    // avoid, so pin the derivation, not just the gate.
    expect(promptSource).toMatch(
      /const\s+rawPendingQ\s*=\s*isAnsweringQuestion\s*\?\s*findLatestPendingQuestion/,
    );
    // Both consumers hang off the ONE optimism-filtered result, so a question
    // the user just answered can neither keep Submit alive nor keep the
    // placeholder asking for an answer.
    expect(promptSource).toMatch(
      /const\s+pendingQ\s*=\s*rawPendingQ\s*&&\s*!pendingAnswers\.has\(rawPendingQ\.toolUseId\)/,
    );
    expect(promptSource).toMatch(/const\s+answeringQuestionCard\s*=\s*pendingQ\s*!==\s*null/);
    expect(promptSource).toMatch(/const\s+pendingMultiQ\s*=\s*pendingQ\?\.multiSelect/);
    // Make sure we did not accidentally leave a second literal compare laying
    // around — the derivation should be the single source of truth.
    const literalHits = promptSource.match(/focusedStatus\s*===\s*'waiting_for_user_answer'/g) ?? [];
    expect(literalHits.length, 'literal compare should appear only inside isAnsweringQuestion').toBe(1);
  });
});
