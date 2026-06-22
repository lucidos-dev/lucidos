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

describe('send-cancel-morph button has tap-gate scroll protection', () => {
  it('imports createTapGate from utils/tapGesture', () => {
    expect(promptSource).toMatch(/import\s*\{[^}]*\bcreateTapGate\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/tapGesture['"]/);
  });

  it('instantiates a tap gate via useMemo so it survives re-renders', () => {
    expect(promptSource).toMatch(/useMemo\(\s*\(\s*\)\s*=>\s*createTapGate\(\)/);
  });

  it('wires onPointerDown to gate.down(clientX, clientY)', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/onPointerDown=\{[^}]*\.down\(\s*[a-zA-Z]+\.clientX\s*,\s*[a-zA-Z]+\.clientY/);
  });

  it('wires onPointerMove to gate.move(clientX, clientY)', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/onPointerMove=\{[^}]*\.move\(\s*[a-zA-Z]+\.clientX\s*,\s*[a-zA-Z]+\.clientY/);
  });

  it('wires onPointerCancel to gate.cancel()', () => {
    const btn = findMorphButton();
    expect(btn).toMatch(/onPointerCancel=\{[^}]*\.cancel\(\)/);
  });

  it('gates the onClick body so isTap() short-circuits the action', () => {
    const btn = findMorphButton();
    // The click handler must call isTap() and bail when it returns false.
    // Tests against a regression where the gate handlers are wired but the
    // click no longer consults them.
    expect(btn).toMatch(/onClick=/);
    expect(btn).toMatch(/\.isTap\(\)/);
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
  it('gates the lone Cancel onClick on isTap() before aborting', () => {
    expect(promptSource).toMatch(/if\s*\(!morphGate\.isTap\(\)\)\s*return;\s*cancelExchangeForTarget\(\)/);
  });

  it('gates the lone Submit onClick on isTap() before sending', () => {
    expect(promptSource).toMatch(/if\s*\(!morphGate\.isTap\(\)\)\s*return;\s*void submit\(\)/);
  });
});
