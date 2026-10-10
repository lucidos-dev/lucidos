/**
 * A typed answer carries its images to the agent (AnswerKind's image_hashes),
 * so the composer no longer refuses an image while a question is open.
 *
 * Static source-pattern checks, like prompt-image-tap.test.ts: mounting
 * PromptInput drags in every chat signal and store.
 */
import { describe, it, expect } from 'vitest';
import { createTapGate, touchActivated } from '../../../utils/tapGesture';
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

describe('every hold turns on side-question mode', () => {
  it('marks the hold so its release does not also act', () => {
    expect(promptSource).toMatch(
      /function releaseEndsHold\(\): boolean \{\s*if \(!heldSendRef\.current\) return false;\s*heldSendRef\.current = false;/,
    );
    // Every new press clears the mark, so it belongs to one gesture only.
    expect(promptSource).toMatch(/onPointerDown: \(e: PointerEvent\) => \{\s*heldSendRef\.current = false;/);
  });

  it('holds only where a side question can be asked', () => {
    expect(promptSource).toMatch(/if \(sideQuestionRefusal !== null\) return;\s*heldSendRef\.current = true;/);
  });

  it('only turns the mode on, and never asks or sends from the hold', () => {
    expect(promptSource).toMatch(/morphGate\.spend\(\);\s*setSideQuestionMode\(true\);\s*\}, \(\) => \{\}\);/);
    expect(promptSource).not.toContain('submit(true)');
    expect(promptSource).toMatch(/function setSideQuestionMode\(on: boolean\): void \{[^]*?updateComposeSelection\(threadId, \{ sideQuestionMode: on \}\);/);
  });

  it('rides on every button that can end the row', () => {
    expect(sendButtonSource()).toContain('{...holdHandlers}');
    expect(submitButtonSource()).toContain('{...holdHandlers}');
    expect(cancelButtonSource()).toContain('{...holdHandlers}');
    expect(promptSource).toContain('primaryPressHandlers={ungatedHoldHandlers}');
  });

  it('keeps the ungated multi-select Submit out of the tap gate', () => {
    const ungated = promptSource.match(/const ungatedHoldHandlers = \{[\s\S]*?\n {2}\};/)?.[0] ?? '';
    expect(ungated, 'ungatedHoldHandlers not found').not.toBe('');
    expect(ungated).not.toContain('morphGate');
  });

  it('spends the mark on every release before it acts', () => {
    expect(promptSource).toMatch(/const morphActivate = useTouchActivated\(\s*\(\) => \{\s*if \(releaseEndsHold\(\)\) return;/);
    expect(promptSource).toMatch(/const answerSubmitActivate = useTouchActivated\(\(\) => \{\s*if \(releaseEndsHold\(\)\) return;\s*void submit\(\);/);
    expect(promptSource).toMatch(
      /const answerCancelActivate = useTouchActivated\(\s*\(\) => \{\s*if \(releaseEndsHold\(\)\) return;\s*cancelExchangeForTarget\(\);/,
    );
    expect(promptSource).toMatch(/onPrimary=\{\(\) => \{\s*if \(releaseEndsHold\(\)\) return;\s*void submitMultiAnswer\(\);/);
  });

  it('spends the tap gate, so a finger drifting during the hold is not a refused swipe', () => {
    expect(promptSource).toMatch(/heldSendRef\.current = true;[^]*?morphGate\.spend\(\);[^]*?setSideQuestionMode\(true\);/);
    // What the spend buys: Stop's destructive lift reaches the hold mark.
    const gate = createTapGate();
    let served = 0;
    const lift = touchActivated(() => { served += 1; }, {
      destructive: () => true,
      gate: { pass: () => gate.tapRejection() === null, spend: gate.spend, aborted: gate.wasAborted },
    });
    gate.down({ screenX: 0, screenY: 0 });
    gate.spend();
    gate.move({ screenX: 12, screenY: 0 });
    lift.onTouchEnd({ preventDefault: () => {} } as never);
    expect(served).toBe(1);
  });

  it('hands focus back to the composer after a mouse hold, for the side question to be typed', () => {
    expect(promptSource).toMatch(/holdRefocusesComposerRef\.current = e\.pointerType === 'mouse';/);
    // After the release's click, which the `.action-btn` blur listener hears.
    expect(promptSource).toMatch(
      /if \(holdRefocusesComposerRef\.current\) requestAnimationFrame\(\(\) => focusIfNeeded\(inputRef\.current\)\);\s*return true;/,
    );
  });

  it('lets a key press act even after a right-click left the hold mark', () => {
    expect(promptSource).toMatch(/onKeyDown: \(e: KeyboardEvent\) => \{\s*if \(e\.key === 'Enter' \|\| e\.key === ' '\) heldSendRef\.current = false;/);
  });

  it('shows a pill whose × leaves the mode and keeps the text', () => {
    expect(promptSource).toMatch(/<Disclosure key="side-question-mode" open=\{sideQuestionMode\}>/);
    expect(promptSource).toMatch(/onClick=\{\(\) => setSideQuestionMode\(false\)\}/);
  });

  it('says so in each tooltip', () => {
    expect(sendButtonSource()).toContain("morphMode === 'cancel' && holdOffersSideQuestion ? tooltipWithShortcut('Stop. Hold for a side question', 'stopThread')");
    expect(sendButtonSource()).toContain("morphMode === 'send' && holdOffersSideQuestion ? tooltipWithShortcut('Send. Hold for a side question', 'askSideQuestion')");
    expect(submitButtonSource()).toContain("holdOffersSideQuestion ? tooltipWithShortcut('Send answer. Hold for a side question', 'askSideQuestion')");
  });
});

describe('in side-question mode the round button asks, in every thread state', () => {
  it('takes over from a waiting card\'s Submit once the box holds text', () => {
    expect(promptSource).toMatch(/const answersCard = isAnsweringQuestion && !\(sideQuestionMode && morphHasContent\);/);
    expect(promptSource).toContain('{answersCard ? answerControl : sendButton}');
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

  it('toggles the mode, and turning it on puts focus in the box', () => {
    expect(promptSource).toMatch(
      /setSideQuestionMode\(!sideQuestionMode\);\s*if \(!sideQuestionMode\) requestAnimationFrame\(\(\) => focusIfNeeded\(inputRef\.current\)\);/,
    );
  });

  it('keeps its chord from also sending the draft', () => {
    expect(promptSource).toMatch(/e\.key === 'Enter' && !e\.shiftKey && !isMobile\(\) && matchShortcut\(e\) === null/);
  });
});
