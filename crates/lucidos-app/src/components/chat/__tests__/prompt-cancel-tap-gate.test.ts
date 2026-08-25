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

/** The body of a `useTouchActivated` call, which is the ONE action both the
 *  touch path and the click path run. Gating it gates every activation. */
function findActivationBody(name: string): string {
  const re = new RegExp(`const ${name} = useTouchActivated\\([\\s\\S]*?\\n  \\}(?:,[^)]*)?\\);`);
  const match = promptSource.match(re);
  if (!match) throw new Error(`${name} not found in PromptInput.tsx`);
  return match[0];
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

  it('gates the activation body so a discarded press short-circuits the action', () => {
    // The action must consult the gate and bail when it says no. Tests against
    // a regression where the gate handlers are wired but the activation no
    // longer consults them. The body left the JSX when the button grew a touch
    // path, and both paths run that one body.
    expect(findMorphButton()).toMatch(/onClick=\{sendActivate\.onClick\}/);
    expect(findActivationBody('sendActivate')).toMatch(/if\s*\(!morphTapPassed\(\)\)\s*return;/);
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
  it('gates the lone Cancel onClick before aborting', () => {
    expect(promptSource).toMatch(/if\s*\(!morphTapPassed\(\)\)\s*return;\s*cancelExchangeForTarget\(\)/);
  });

  it('gates the lone Submit onClick before sending', () => {
    expect(promptSource).toMatch(/if\s*\(!morphTapPassed\(\)\)\s*return;\s*void submit\(\)/);
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
// Constructive actions only, by decision. A dropped tap on Stop or Cancel is a
// safe no-op the user repeats. Firing those a gesture earlier would work
// against the gate and the settle window above.
const splitButtonSource = readFileSync(resolve(here, '../../shared/SplitButton.tsx'), 'utf-8');
const bannerSource = readFileSync(resolve(here, '../WaitingBanner.tsx'), 'utf-8');

describe('the prompt row survives the iOS keyboard dropping a click', () => {
  it('binds the handlers through the hook, so the twin window survives a render', () => {
    expect(promptSource).toMatch(/import\s*\{[^}]*\buseTouchActivated\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/hooks\/useTouchActivated['"]/);
  });

  it('gives the morph Send a touch path', () => {
    expect(findMorphButton()).toMatch(/onTouchEnd=\{sendActivate\.onTouchEnd\}/);
  });

  it('enables that touch path in Send mode only, never on Stop or Cancel', () => {
    expect(findActivationBody('sendActivate')).toMatch(/\}, morphMode === 'send'\);$/);
  });

  it('gives the lone answer Submit a touch path, gated the same way', () => {
    expect(promptSource).toMatch(/onTouchEnd=\{answerSubmitActivate\.onTouchEnd\}/);
    expect(findActivationBody('answerSubmitActivate')).toMatch(/if\s*\(!morphTapPassed\(\)\)\s*return;/);
  });

  it('opts the multi-select Submit in through the split button', () => {
    expect(promptSource).toMatch(/primaryTouchActivate/);
    expect(splitButtonSource).toMatch(/!!props\.primaryTouchActivate && !props\.primaryDisabled/);
  });

  it('leaves the destructive buttons on click alone', () => {
    // Exactly two touch paths in the row: the morph in Send mode, and the lone
    // answer Submit. A third means a destructive button took one.
    expect((promptSource.match(/onTouchEnd=/g) ?? []).length).toBe(2);
  });

  it('leaves the change-action banner Apply on click', () => {
    // The split button is shared. Touch activation is opt-in precisely so the
    // banner, which nobody reaches with a keyboard up, is unchanged.
    expect(bannerSource).not.toMatch(/primaryTouchActivate/);
  });

  it('blurs from inside each touch-activated action', () => {
    // The suppressed click never reaches `installActionBtnBlurListener`, which
    // listens on `click`. So each action has to drop the keyboard itself.
    const submitFn = promptSource.match(/async function submit\(\)[\s\S]*?\n  \}/);
    const multiFn = promptSource.match(/async function submitMultiAnswer\(\)[\s\S]*?\n  \}/);
    expect(submitFn, 'submit() not found').not.toBeNull();
    expect(multiFn, 'submitMultiAnswer() not found').not.toBeNull();
    expect(submitFn![0]).toMatch(/if \(isMobile\(\)\) el\.blur\(\)/);
    expect(multiFn![0]).toMatch(/if \(isMobile\(\)\) el\?\.blur\(\)/);
  });

  it('never returns from submit without saying something', () => {
    // The queued-upload branch used to return with nothing on screen, which is
    // the exact shape this whole change is about.
    expect(promptSource).toMatch(/queueUploadSend\(threadId, \{ useCodingAgent, context \}\);[\s\S]{0,300}?showToast\(UPLOAD_QUEUED_SEND_TOAST/);
  });
});
