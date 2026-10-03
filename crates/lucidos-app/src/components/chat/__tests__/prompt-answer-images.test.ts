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

describe('a hold on Send asks the draft as a side question', () => {
  it('marks the hold so its release does not also send', () => {
    expect(promptSource).toMatch(
      /function releaseEndsHold\(\): boolean \{\s*if \(!heldSendRef\.current\) return false;\s*heldSendRef\.current = false;/,
    );
    expect(promptSource).toMatch(/const morphActivate = useTouchActivated\(\s*\(\) => \{\s*if \(releaseEndsHold\(\)\) return;/);
    // Every new press clears the mark, so it belongs to one gesture only.
    expect(promptSource).toMatch(/onPointerDown: \(e: PointerEvent\) => \{\s*heldSendRef\.current = false;/);
  });

  it('offers the menu only where a side question can be asked', () => {
    expect(promptSource).toMatch(/if \(!canAskFromHold\) return;\s*heldSendRef\.current = true;/);
    expect(promptSource).toMatch(/const canAskFromHold = sideQuestionBlocker === null;/);
    expect(promptSource).toMatch(/\} else void submit\(true\);/);
  });

  it('a closed menu clears the mark, so a later Send is never swallowed', () => {
    expect(promptSource).toMatch(/onClosed=\{\(opener\) => \{\s*heldSendRef\.current = false;/);
  });
});

describe('a hold on Stop starts a side question', () => {
  it('offers it on the morph Stop and on a waiting card\'s lone Cancel', () => {
    expect(promptSource).toMatch(
      /stopOrCancelShown: \(morphMode === 'cancel' && !isAnsweringQuestion\) \|\| answerMode === 'cancel',/,
    );
  });

  // The lone Cancel carries the same hold, and its release never also cancels.
  it('gives the waiting card\'s Cancel the hold, spent before it cancels', () => {
    const cancel = promptSource.match(/answerMode === 'cancel' \? \([\s\S]*?<\/button>/)?.[0] ?? '';
    expect(cancel).toContain('{...holdHandlers}');
    expect(promptSource).toMatch(
      /const answerCancelActivate = useTouchActivated\(\s*\(\) => \{\s*if \(releaseEndsHold\(\)\) return;[^]*?cancelExchangeForTarget\(\);/,
    );
  });

  it('turns on side-question mode over the empty composer instead of asking', () => {
    expect(promptSource).toMatch(
      /if \(holdSideQuestion\.kind === 'start-mode'\) \{\s*startedSideQuestionModeRef\.current = true;\s*setSideQuestionMode\(true\);/,
    );
    expect(promptSource).toMatch(/function setSideQuestionMode\(on: boolean\): void \{[^]*?updateComposeSelection\(threadId, \{ sideQuestionMode: on \}\);/);
  });

  it('shows a pill whose × leaves the mode and keeps the text', () => {
    expect(promptSource).toMatch(/<Disclosure key="side-question-mode" open=\{sideQuestionMode\}>/);
    expect(promptSource).toMatch(/onClick=\{\(\) => setSideQuestionMode\(false\)\}/);
  });

  it('skips the pill for a mouse hold, so the box takes typing at once', () => {
    expect(promptSource).toMatch(
      /if \(holdSideQuestion\.kind === 'start-mode' && pressPointerTypeRef\.current === 'mouse'\) \{\s*setSideQuestionMode\(true\);\s*holdRefocusesComposerRef\.current = true;\s*return;\s*\}\s*sendHoldMenuOpener\.value = 'hold';/,
    );
    expect(promptSource).toMatch(/pressPointerTypeRef\.current = e\.pointerType;/);
  });

  it('marks the hold so its release does not also stop the turn', () => {
    // The same mark as Send's, checked before the morph picks send or cancel.
    const activate = promptSource.match(/const morphActivate = useTouchActivated\([\s\S]*?\n {2}\);/)?.[0] ?? '';
    expect(activate.indexOf('if (releaseEndsHold())')).toBeGreaterThan(-1);
    expect(activate.indexOf('if (releaseEndsHold())')).toBeLessThan(activate.indexOf('cancelExchangeForTarget()'));
  });

  it('spends the tap gate, so a finger drifting under the pill is not a refused swipe', () => {
    // Before the branch, so a hold that turns on the mode spends it too.
    expect(promptSource).toMatch(/heldSendRef\.current = true;[^]*?morphGate\.spend\(\);[^]*?if \(holdSideQuestion\.kind === 'start-mode' &&[^]*?sendHoldMenuOpener\.value = 'hold';/);
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

  it('shuts the pill when Stop stops', () => {
    expect(promptSource).toMatch(
      /else if \(morphMode === 'cancel'\) \{\s*sendHoldMenuOpener\.value = null;\s*cancelExchangeForTarget\(\);/,
    );
  });

  it('says so in Stop\'s tooltip', () => {
    expect(promptSource).toContain("morphMode === 'cancel' && offersHoldHint ? tooltipWithShortcut('Stop. Hold to ask a side question', 'stopThread')");
  });

  it('hands focus back to the composer when a mouse hold began mid-typing', () => {
    expect(promptSource).toMatch(
      /holdRefocusesComposerRef\.current = e\.pointerType === 'mouse' && document\.activeElement === inputRef\.current;/,
    );
    expect(promptSource).toMatch(/sendHoldMenuOpener\.value = 'hold';\s*if \(holdRefocusesComposerRef\.current\) focusIfNeeded\(inputRef\.current\);/);
    // Again after the release's click, which the `.action-btn` blur listener hears.
    expect(promptSource).toMatch(
      /if \(holdRefocusesComposerRef\.current\) requestAnimationFrame\(\(\) => focusIfNeeded\(inputRef\.current\)\);\s*return true;/,
    );
  });

  it('lets Enter in the composer press the Side question half while the pill is open', () => {
    expect(promptSource).toMatch(
      /else if \(sendHoldMenuOpener\.value !== null\) \{\s*sendHoldMenuOpener\.value = null;\s*askFromSideQuestionPill\(\);\s*\} else void submit\(\);/,
    );
    expect(promptSource).toContain('onAskSideQuestion={askFromSideQuestionPill}');
  });

  it('drops the hold hint from both tooltips while the pill is open', () => {
    expect(promptSource).toMatch(/const offersHoldHint = canAskFromHold && sendHoldMenuOpener\.value === null;/);
    expect(promptSource).toContain("morphMode === 'send' && offersHoldHint ? tooltipWithShortcut('Send. Hold to ask a side question', 'askSideQuestion')");
    expect(promptSource).not.toMatch(/canAskFromHold \? tooltipWithShortcut/);
  });
});

describe('Send is the split pill\'s other half', () => {
  it('anchors the pill, so a tap on Send sends instead of only dismissing', () => {
    expect(promptSource).toMatch(/<SendHoldMenu\s+anchor=\{splitPillButtonEl\}/);
    expect(sendButtonSource()).toMatch(/ref=\{setSplitPillButtonEl\}\s*\{\.\.\.holdHandlers\}/);
  });

  it('wraps the half and the end button alone, so the half measures that button', () => {
    expect(promptSource).toMatch(
      /<span class="split-pill">\s*<SendHoldMenu[\s\S]*?\/>\s*\{isAnsweringQuestion \? answerControl : sendButton\}\s*<\/span>/,
    );
  });

  it('shuts the pill when Send sends', () => {
    expect(promptSource).toMatch(
      /if \(morphMode === 'send'\) \{\s*sendHoldMenuOpener\.value = null;\s*void submit\(\);\s*\}/,
    );
  });

  it('comes after the Side question half, so Tab moves from that half to Send', () => {
    const half = promptSource.indexOf('<SendHoldMenu');
    const send = promptSource.indexOf('{isAnsweringQuestion ? answerControl : sendButton}');
    expect(half).toBeGreaterThan(-1);
    expect(half).toBeLessThan(send);
  });

  it('lets a key press send even after a right-click left the hold mark', () => {
    expect(promptSource).toMatch(/onKeyDown: \(e: KeyboardEvent\) => \{\s*if \(e\.key === 'Enter' \|\| e\.key === ' '\) heldSendRef\.current = false;/);
  });

  it('shuts the pill once the button under it offers no side question', () => {
    expect(promptSource).toMatch(/if \(sideQuestionBlocker !== null\) sendHoldMenuOpener\.value = null;/);
  });

  it('squares off while the pill is open, and until its half has slid back', () => {
    expect(promptSource).toMatch(/sendHoldMenuShown \? ' split-open' : ''/);
    expect(promptSource).toMatch(/leaving=\{sendHoldMenuShown && sendHoldMenuOpener\.value === null\}/);
  });
});

describe('Submit is the split pill\'s other half while a question card waits', () => {
  it('anchors the pill and takes the same hold as Send', () => {
    const submit = submitButtonSource();
    expect(submit, 'the lone Submit not found').not.toBe('');
    expect(submit).toMatch(/ref=\{setSplitPillButtonEl\}\s*\{\.\.\.holdHandlers\}/);
  });

  it('marks the hold so its release does not also answer', () => {
    expect(promptSource).toMatch(
      /const answerSubmitActivate = useTouchActivated\(\(\) => \{\s*if \(releaseEndsHold\(\)\) return;\s*sendHoldMenuOpener\.value = null;\s*void submit\(\);/,
    );
  });

  it('squares off while the pill is open', () => {
    expect(submitButtonSource()).toMatch(/class=\{'action-btn action-btn-confirm' \+ \(sendHoldMenuShown \? ' split-open' : ''\)\}/);
  });

  it('says so in its tooltip', () => {
    expect(submitButtonSource()).toContain(
      "offersHoldHint ? tooltipWithShortcut('Send answer. Hold to ask a side question', 'askSideQuestion')",
    );
  });

  it('keeps the anchor on the lone node whichever face it wears', () => {
    // Submit, Cancel and Canceling share one node. Preact clears a ref only
    // when the vnode carries one. A face without it leaves the anchor on a node
    // that later unmounts.
    const faces = promptSource.match(/<button\s+key="answer-lone"[\s\S]*?>/g) ?? [];
    expect(faces).toHaveLength(3);
    for (const face of faces) expect(face).toContain('ref={setSplitPillButtonEl}');
  });
});

describe('the Side question shortcut', () => {
  it('opens the menu only where a hold could, and says why elsewhere', () => {
    expect(promptSource).toMatch(
      /else if \(sideQuestionBlocker === null\) sendHoldMenuOpener\.value = 'shortcut';\s*else showToast\(sideQuestionBlocker, 'info'\);/,
    );
  });

  it('turns on side-question mode over an empty box, so the user types straight on', () => {
    expect(promptSource).toMatch(
      /else if \(holdSideQuestion\.kind === 'start-mode'\) setSideQuestionMode\(true\);\s*else if \(sideQuestionBlocker === null\) sendHoldMenuOpener\.value = 'shortcut';/,
    );
  });

  it('shuts the open pill on a second press, as Escape does', () => {
    expect(promptSource).toMatch(
      /promptSideQuestionRequested\.value = false;[^]*?if \(sendHoldMenuOpener\.peek\(\) !== null\) sendHoldMenuOpener\.value = null;\s*else if \(holdSideQuestion\.kind === 'start-mode'\)/,
    );
  });

  it('works during a running turn, since a typed draft turns Stop back into Send', () => {
    expect(promptSource).toMatch(/hasContent: morphMode === 'send',/);
  });

  it('hands focus back to the draft when the menu it opened shuts, as a started side question does', () => {
    // A frame late, after the document-level `.action-btn` click blur.
    expect(promptSource).toMatch(
      /if \(opener === 'shortcut' \|\| startedMode\) requestAnimationFrame\(\(\) => inputRef\.current\?\.focus\(\)\);/,
    );
  });

  it('keeps its chord from also sending the draft', () => {
    expect(promptSource).toMatch(/e\.key === 'Enter' && !e\.shiftKey && !isMobile\(\) && matchShortcut\(e\) === null/);
  });
});
