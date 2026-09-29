/**
 * A typed answer carries its images to the agent (AnswerKind's image_hashes),
 * so the composer no longer refuses an image while a question is open.
 *
 * Static source-pattern checks, like prompt-image-tap.test.ts: mounting
 * PromptInput drags in every chat signal and store.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

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
    // An upload still running would answer without its image, so it waits.
    expect(fn.indexOf('hasInFlightUploads(focused)')).toBeLessThan(fn.indexOf('answerThreadQuestion('));
  });

  it('images alone can submit a multi-select answer', () => {
    expect(promptSource).toMatch(/submitMultiDisabled = submitMultiCount === 0 && images\.length === 0/);
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
      /const\s+pendingQ\s*=\s*rawPendingQ\s*&&\s*!pendingAnswers\.has\(rawPendingQ\.toolUseId\)/,
    );
    expect(promptSource).toMatch(/const\s+answeringQuestionCard\s*=\s*pendingQ\s*!==\s*null/);
    expect(promptSource).toMatch(/const\s+pendingMultiQ\s*=\s*pendingQ\?\.multiSelect/);
    const literalHits = promptSource.match(/focusedStatus\s*===\s*'waiting_for_user_answer'/g) ?? [];
    expect(literalHits.length, 'literal compare should appear only inside isAnsweringQuestion').toBe(1);
  });
});

describe('a hold on Send asks the draft as a side question', () => {
  it('marks the hold so its release does not also send', () => {
    expect(promptSource).toMatch(
      /const morphActivate = useTouchActivated\(\s*\(\) => \{\s*if \(heldSendRef\.current\) \{\s*heldSendRef\.current = false;\s*return;/,
    );
    // Every new press clears the mark, so it belongs to one gesture only.
    expect(promptSource).toMatch(/onPointerDown=\{e => \{\s*heldSendRef\.current = false;/);
  });

  it('offers the menu only where a side question can be asked', () => {
    expect(promptSource).toMatch(/if \(!canAskFromHold\) return;\s*heldSendRef\.current = true;/);
    expect(promptSource).toMatch(/const canAskFromHold = [^;]*promptCodingAgent !== 'codex';/);
    expect(promptSource).toContain('onAskSideQuestion={() => void submit(true)}');
  });

  it('a closed menu clears the mark, so a later Send is never swallowed', () => {
    expect(promptSource).toMatch(/onClosed=\{\(\) => \{ heldSendRef\.current = false; \}\}/);
  });
});
