import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

// Regression for an iOS PWA bug where the morph Send→Cancel button fired
// `handleCancelExchange` from a scroll-tap (touch that stayed under iOS's
// native cancel threshold), silently aborting a `waiting_for_user_answer`
// turn and writing `UserQuestionAnswered { kind: Canceled }` + a
// `ResponseCanceled { cause: user_stop }`. The fix mirrors QuestionCard's
// `OptionButton` — wire the button through a `createTapGate` so
// `pointerdown`+`pointermove` past the 8 px threshold suppresses the
// resulting click.
const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

function findMorphButton(): string {
  const match = promptSource.match(/<button\s+key="send-cancel-morph"[\s\S]*?<\/button>/);
  if (!match) throw new Error('send-cancel-morph button not found in PromptInput.tsx');
  return match[0];
}

/** The whole `useTouchActivated` call, which carries the ONE action both paths
 *  run, plus the enable flag, the gate and the destructive flag.
 *
 *  Found by balancing parentheses rather than by a shape regex. A regex keyed
 *  on the old one-line formatting matched nothing once the call was wrapped,
 *  and an assertion over nothing passes. */
function findActivationBody(name: string): string {
  const start = promptSource.indexOf(`const ${name} = useTouchActivated(`);
  if (start < 0) throw new Error(`${name} not found in PromptInput.tsx`);
  let depth = 0;
  for (let i = promptSource.indexOf('(', start); i < promptSource.length; i++) {
    if (promptSource[i] === '(') depth++;
    else if (promptSource[i] === ')' && --depth === 0) return promptSource.slice(start, i + 1);
  }
  throw new Error(`${name} call is unbalanced in PromptInput.tsx`);
}

describe('send-cancel-morph button has tap-gate scroll protection', () => {
  it('imports createTapGate from utils/tapGesture', () => {
    expect(promptSource).toMatch(/import\s*\{[^}]*\bcreateTapGate\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/tapGesture['"]/);
  });

  it('instantiates a tap gate via useMemo so it survives re-renders', () => {
    expect(promptSource).toMatch(/useMemo\(\s*\(\s*\)\s*=>\s*createTapGate\(\)/);
  });

  it('wires onPointerDown to gate.down(event)', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/onPointerDown=\{\s*[a-zA-Z]+\s*=>\s*morphGate\.down\(\s*[a-zA-Z]+\s*\)/);
  });

  it('wires onPointerMove to gate.move(event)', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/onPointerMove=\{\s*[a-zA-Z]+\s*=>\s*morphGate\.move\(\s*[a-zA-Z]+\s*\)/);
  });

  it('wires onPointerCancel to gate.cancel()', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/onPointerCancel=\{[^}]*\.cancel\(\)/);
  });

  it('hands the gate to every path that could fire from a scroll', () => {
    // The gate catches the click iOS fires after a touch that was starting a
    // scroll. It used to wrap the shared action, where it also vetoed the
    // constructive touch path. That path is the only one on iOS with the
    // keyboard up, so a veto there is a dead button. So `touchActivated` asks
    // the gate on `onClick`, and on `onTouchEnd` only for a destructive face.
    expect(findMorphButton()).toMatch(/onClick=\{morphActivate\.onClick\}/);
    expect(findActivationBody('morphActivate')).toMatch(/morphActivationGate/);
  });

  it('gives that gate a spend half, so a served press cannot rule on the next', () => {
    // The gate holds ONE press, and the touch path serves without asking. So
    // it spends the press instead. A stale one would rule on the next
    // activation that arrives with none of its own.
    expect(promptSource).toMatch(/spend: morphGate\.spend,/);
  });

  it('never wires spend to cancel, which would fake an aborted gesture', () => {
    // They were one method. `cancel` means the SYSTEM took the gesture, which
    // `aborted` reports to the destructive touch path. A served press sharing
    // it would stand the next Cancel down.
    expect(promptSource).not.toMatch(/spend: morphGate\.cancel/);
    expect(promptSource).toMatch(/aborted: morphGate\.wasAborted,/);
  });

  it('does not open a confirmation dialog before canceling', () => {
    expect(findMorphButton()).not.toMatch(/showConfirm\(/);
  });
});

// During `waiting_for_user_answer` the prompt row swaps the morph button for
// the answer control (Submit-default; lone Cancel when nothing's submittable),
// so the destructive one-tap Cancel — and the lone Submit — must carry the SAME
// scroll-vs-tap gate. Without it the iOS scroll-tap turn-abort regression simply
// moves to the relocated button.
describe('answer control Cancel/Submit have tap-gate scroll protection', () => {
  it('routes the lone Cancel through a destructive activation, which is gated', () => {
    // It used to guard its click inline. The shared path now carries the gate
    // on both of its paths, because the button gained a touch path.
    const body = findActivationBody('answerCancelActivate');
    expect(body).toMatch(/cancelExchangeForTarget\(\)/);
    expect(body).toMatch(/morphActivationGate/);
    expect(body).toMatch(/true,\s*\)$/);
  });

  it('gates the lone Submit on its click path', () => {
    // Through `useTouchActivated`'s gate argument rather than inline, for the
    // reason the morph button's case above gives.
    expect(findActivationBody('answerSubmitActivate')).toMatch(/\}, true, morphActivationGate\)$/);
  });
});

// The gate reads SCREEN coordinates, and it reads them itself from the event
// so no call site can hand it another space. Client coordinates track the
// finger against the page viewport. An iOS keyboard offsets that viewport
// under a stationary finger, and the gate read the shift as a swipe.
//
// A discarded tap is also the user's press thrown away, which the frontend's
// no-hidden-errors rule forbids doing silently: the button just looks dead.
describe('the composer gate measures the finger and never fails silently', () => {
  it('never hands the gate client coordinates', () => {
    expect(promptSource).not.toMatch(/morphGate\.(down|move)\([^)]*client[XY]/);
  });

  it('routes every gated click through morphTapPassed, never the silent predicate', () => {
    // `isTap()` is the silent entry point, and it belongs to the question
    // card. A composer button on it discards a press without saying so.
    expect(promptSource).not.toMatch(/morphGate\.isTap\(\)/);
    expect((promptSource.match(/morphGate\.tapRejection\(\)/g) ?? []).length).toBe(1);
  });

  it('toasts the discarded press rather than returning silently', () => {
    const helper = promptSource.match(/function morphTapPassed\(\)[\s\S]*?\n  \}/);
    expect(helper, 'morphTapPassed() not found in PromptInput.tsx').not.toBeNull();
    expect(helper![0]).toMatch(/tapRejection\(\)/);
    expect(helper![0]).toMatch(/showToast\(/);
  });
});

// The scroll-vs-tap gate stops a *moving* touch; it does NOT stop a laggy
// *repeat* tap on the same spot after the constructive Send/Submit morphs in
// place into the destructive Cancel/Stop. That's the post-submit settle window
// (armCancelSettle / isCancelSettling): a constructive tap arms it, and while it
// is active the morphed Cancel/Stop renders disabled and the cancel path bails.
// Source-grep tripwires so removing any leg of the protection fails loudly.
describe('post-submit cancel settle window is wired', () => {
  it('arms the settle window when a message/answer is sent', () => {
    // Both the normal send path (submit) and the multi-select answer path.
    expect((promptSource.match(/armCancelSettle\(\)/g) ?? []).length).toBeGreaterThanOrEqual(2);
  });

  it('belts the shared cancel helper with the settle check', () => {
    expect(promptSource).toMatch(/function cancelExchangeForTarget\(\)\s*\{[\s\S]*?if\s*\(isCancelSettling\(\)\)\s*return;/);
  });

  it('disables the morph Stop while settling', () => {
    expect(promptSource).toMatch(/morphMode === 'cancel' \? cancelSettling/);
  });

  it('disables the answer-control lone Cancel while settling', () => {
    expect(promptSource).toMatch(/disabled=\{cancelSettling\}/);
  });
});

// The reported bug, and the sibling of the gate above. The gate decides whether
// a press counts; this decides whether the press reaches a handler at all.
// Tapping a button with the mobile keyboard up can blur the textarea. The
// keyboard dismissal then moves the button out from under the finger before
// WebKit dispatches the synthetic click. The click is dropped and the button
// reads as dead. `touchActivated` runs the action inside the gesture instead.
//
// It was constructive actions only, on the reading that a dropped tap on Cancel
// is a safe no-op the user repeats. It is not: with the keyboard up there is no
// click to repeat to, so the button is simply dead. A destructive face now
// takes the touch path and RULES on the gate, which keeps the protection that
// withholding it was buying.
const splitButtonSource = readFileSync(resolve(here, '../../shared/SplitButton.tsx'), 'utf-8');
const bannerSource = readFileSync(resolve(here, '../WaitingBanner.tsx'), 'utf-8');

describe('the prompt row survives the iOS keyboard dropping a click', () => {
  it('binds the handlers through the hook, so the twin window survives a render', () => {
    expect(promptSource).toMatch(/import\s*\{[^}]*\buseTouchActivated\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/hooks\/useTouchActivated['"]/);
  });

  it('gives the morph a touch path', () => {
    expect(findMorphButton()).toMatch(/onTouchEnd=\{morphActivate\.onTouchEnd\}/);
  });

  it('keeps that touch path live in Send AND Cancel mode', () => {
    // Cancel had none, and iOS drops the click when the keyboard dismisses
    // under the finger, so the button was dead whenever the keyboard was up.
    // The probe logged it as `Cancel: dead` with the finger still and the node
    // connected. See `docs/plans/2026-08-28-cancel-survives-the-ios-keyboard.md`.
    expect(findActivationBody('morphActivate'))
      .toMatch(/morphMode === 'send' \|\| \(morphMode === 'cancel' && !cancelSettling\)/);
  });

  it('marks the morph destructive while it reads Cancel, never while it reads Send', () => {
    // The flag is what makes the touch path RULE on the gate rather than spend
    // it. On Send it must stay false: two shipped fixes asked the constructive
    // path a question, and both were reported as a dead Send.
    expect(findActivationBody('morphActivate')).toMatch(/morphMode === 'cancel',\s*\)$/);
  });

  it('gives the lone answer Submit a touch path', () => {
    expect(promptSource).toMatch(/onTouchEnd=\{answerSubmitActivate\.onTouchEnd\}/);
  });

  it('never cancels mousedown on a face in this row', () => {
    // A `preventDefault()` here holds focus, which is why it reads as the
    // obvious repair for a dropped click. On iOS a cancelled event stops the
    // rest of the synthesized sequence, `click` included, so it removes the
    // fallback it was reaching for. It shipped once and the button went dead
    // wherever the user pressed, until they dismissed the keyboard.
    for (const source of [promptSource, splitButtonSource, bannerSource]) {
      expect(source).not.toMatch(/onMouseDown=/);
      expect(source).not.toMatch(/\bholdFocusOnPress\b/);
    }
  });

  it('gives Diff a touch path, since it sits in the row and is not destructive', () => {
    // Reported dead with the keyboard up, alongside the answer Submit and the
    // lone Cancel. Diff opens a view and can be repeated, so none of the
    // reasons the destructive faces decline the touch path apply to it.
    expect(bannerSource).toMatch(/onTouchEnd=\{activate\.onTouchEnd\}/);
    expect(bannerSource).toMatch(/import\s*\{[^}]*\buseTouchActivated\b[^}]*\}/);
  });

  it('opts the multi-select Submit in through the split button', () => {
    expect(promptSource).toMatch(/primaryTouchActivate/);
    expect(splitButtonSource).toMatch(/!!props\.primaryTouchActivate && !props\.primaryDisabled/);
  });

  it('enumerates every touch path in the row, and marks each destructive one', () => {
    // The row's touch paths are enumerated, never counted to a bare total. A
    // new one then has to be named here, and say whether it is destructive.
    // Three live in PromptInput: the morph, the lone answer Submit and the
    // lone answer Cancel. Diff is the fourth and lives in WaitingBanner.
    expect((promptSource.match(/onTouchEnd=/g) ?? []).length).toBe(3);
    expect((bannerSource.match(/onTouchEnd=/g) ?? []).length).toBe(1);
    expect(findMorphButton()).toMatch(/onTouchEnd=\{morphActivate\.onTouchEnd\}/);
    const loneCancel = promptSource.match(/answerMode === 'cancel' \?[\s\S]*?<\/button>/);
    expect(loneCancel, 'the lone answer Cancel was not found').not.toBeNull();
    expect(loneCancel![0]).toMatch(/onTouchEnd=\{answerCancelActivate\.onTouchEnd\}/);
    // The two destructive ones pass the flag; the two constructive ones do not.
    expect(findActivationBody('morphActivate')).toMatch(/morphMode === 'cancel',\s*\)$/);
    expect(findActivationBody('answerCancelActivate')).toMatch(/true,\s*\)$/);
    expect(findActivationBody('answerSubmitActivate')).not.toMatch(/destructive/);
    expect(bannerSource).toMatch(/const activate = useTouchActivated\(\(\) => \{[\s\S]*?\n  \}\);/);
  });

  it('leaves the change-action banner Apply on click', () => {
    // The split button is shared, and touch activation is opt-in. Apply leaves
    // it off by choice, not by reach: it renders into this same row, but a
    // press sliding off it would merge a branch.
    expect(bannerSource).not.toMatch(/primaryTouchActivate/);
  });

  it('blurs from inside each touch-activated action', () => {
    // The suppressed click never reaches `installActionBtnBlurListener`, which
    // listens on `click`. So each action has to drop the keyboard itself.
    const submitFn = promptSource.match(/async function submit\(\)[\s\S]*?\n  \}/);
    const multiFn = promptSource.match(/async function submitMultiAnswer\(\)[\s\S]*?\n  \}/);
    const diffFn = bannerSource.match(/function DiffButton\([\s\S]*?\n\}/);
    expect(submitFn, 'submit() not found').not.toBeNull();
    expect(multiFn, 'submitMultiAnswer() not found').not.toBeNull();
    expect(diffFn, 'DiffButton not found').not.toBeNull();
    expect(diffFn![0]).toMatch(/blurPromptInputIfFocused\(\)/);
    // Optional on both, because a send no longer needs the textarea node. See
    // `resolveComposerText`.
    expect(submitFn![0]).toMatch(/if \(isMobile\(\)\) el\?\.blur\(\)/);
    expect(multiFn![0]).toMatch(/if \(isMobile\(\)\) el\?\.blur\(\)/);
  });

  it('never returns from submit without saying something', () => {
    // The queued-upload branch used to return with nothing on screen, which is
    // the exact shape this whole change is about.
    expect(promptSource).toMatch(/queueUploadSend\(threadId, \{ useCodingAgent, context \}\);[\s\S]{0,300}?showToast\(UPLOAD_QUEUED_SEND_TOAST/);
  });
});
