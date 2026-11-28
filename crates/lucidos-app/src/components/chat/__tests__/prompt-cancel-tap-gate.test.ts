import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

function findMorphButton(): string {
  const match = promptSource.match(/<button\s+key="send-cancel-morph"[\s\S]*?<\/button>/);
  if (!match) throw new Error('send-cancel-morph button not found in PromptInput.tsx');
  return match[0];
}

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

function findTapGateHandlers(): string {
  const start = promptSource.indexOf('const tapGateHandlers = {');
  if (start < 0) throw new Error('tapGateHandlers not found in PromptInput.tsx');
  let depth = 0;
  for (let i = promptSource.indexOf('{', start); i < promptSource.length; i++) {
    if (promptSource[i] === '{') depth++;
    else if (promptSource[i] === '}' && --depth === 0) return promptSource.slice(start, i + 1);
  }
  throw new Error('tapGateHandlers is unbalanced in PromptInput.tsx');
}

describe('send-cancel-morph button has tap-gate scroll protection', () => {
  it('imports createTapGate from utils/tapGesture', () => {
    expect(promptSource).toMatch(/import\s*\{[^}]*\bcreateTapGate\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/tapGesture['"]/);
  });

  it('instantiates a tap gate via useMemo so it survives re-renders', () => {
    expect(promptSource).toMatch(/useMemo\(\s*\(\s*\)\s*=>\s*createTapGate\(\)/);
  });

  it('takes its pointer handlers from the shared tap gate handlers', () => {
    expect(findMorphButton()).toMatch(/\{\.\.\.tapGateHandlers\}/);
  });

  it('wires onPointerDown to gate.down(event)', () => {
    expect(findTapGateHandlers()).toMatch(/onPointerDown: \(([a-zA-Z]+): PointerEvent\) => morphGate\.down\(\s*\1\s*\)/);
  });

  it('wires onPointerMove to gate.move(event)', () => {
    expect(findTapGateHandlers()).toMatch(/onPointerMove: \(([a-zA-Z]+): PointerEvent\) => morphGate\.move\(\s*\1\s*\)/);
  });

  it('wires onPointerCancel to gate.cancel()', () => {
    expect(findTapGateHandlers()).toMatch(/onPointerCancel: \(\) => morphGate\.cancel\(\)/);
  });

  it('hands the gate to every path that could fire from a scroll', () => {
    expect(findMorphButton()).toMatch(/onClick=\{morphActivate\.onClick\}/);
    expect(findActivationBody('morphActivate')).toMatch(/morphActivationGate/);
  });

  it('gives that gate a spend half, so a served press cannot rule on the next', () => {
    expect(promptSource).toMatch(/spend: morphGate\.spend,/);
  });

  it('never wires spend to cancel, which would fake an aborted gesture', () => {
    expect(promptSource).not.toMatch(/spend: morphGate\.cancel/);
    expect(promptSource).toMatch(/aborted: morphGate\.wasAborted,/);
  });

  it('does not open a confirmation dialog before canceling', () => {
    expect(findMorphButton()).not.toMatch(/showConfirm\(/);
  });
});

describe('answer control Cancel/Submit have tap-gate scroll protection', () => {
  it('routes the lone Cancel through a destructive activation, which is gated', () => {
    const body = findActivationBody('answerCancelActivate');
    expect(body).toMatch(/cancelExchangeForTarget\(\)/);
    expect(body).toMatch(/morphActivationGate/);
    expect(body).toMatch(/true,\s*answerShownAt,\s*\)$/);
  });

  it('gates the lone Submit on its click path', () => {
    expect(findActivationBody('answerSubmitActivate')).toMatch(/\}, true, morphActivationGate\)$/);
  });

  it('feeds the lone Submit\'s pointer to the gate through the shared tap gate handlers', () => {
    const submit = promptSource.match(/answerMode === 'submit' \? \([\s\S]*?<\/button>/)?.[0] ?? '';
    expect(submit, 'the lone Submit not found').not.toBe('');
    expect(submit).toMatch(/\{\.\.\.tapGateHandlers\}/);
  });
});

describe('the composer gate measures the finger and never fails silently', () => {
  it('never hands the gate client coordinates', () => {
    expect(promptSource).not.toMatch(/morphGate\.(down|move)\([^)]*client[XY]/);
  });

  it('routes every gated click through morphTapPassed, never the silent predicate', () => {
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

describe('post-submit cancel settle window is wired', () => {
  it('arms the settle window when a message/answer is sent', () => {
    expect((promptSource.match(/armCancelSettle\(\)/g) ?? []).length).toBeGreaterThanOrEqual(2);
  });

  it('belts the shared cancel helper with the settle check', () => {
    expect(promptSource).toMatch(/function cancelExchangeForTarget\(\)\s*\{[\s\S]*?if\s*\(isCancelSettling\(\)\)\s*return;/);
  });

  it('disables the morph Stop while settling', () => {
    expect(promptSource).toMatch(/morphMode === 'cancel' \? cancelSettling/);
  });

  it('keeps the settling Stop at full strength', () => {
    expect(findMorphButton()).toMatch(/morphMode === 'cancel' && cancelSettling \? ' morph-settling'/);
    const hostCss = readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8');
    expect(hostCss).toMatch(/\.send-cancel-morph\.morph-settling:disabled\s*\{\s*opacity:\s*1;\s*\}/);
  });

  it('disables the answer-control lone Cancel while settling', () => {
    expect(promptSource).toMatch(/disabled=\{cancelSettling\}/);
  });
});

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
    expect(findActivationBody('morphActivate'))
      .toMatch(/morphMode === 'send' \|\| \(morphMode === 'cancel' && !cancelSettling\)/);
  });

  it('marks the morph destructive while it reads Cancel, never while it reads Send', () => {
    expect(findActivationBody('morphActivate')).toMatch(/morphMode === 'cancel',\s*morphShownAt,\s*\)$/);
  });

  it('gives each destructive face its paint time, keyed on the face it shows', () => {
    expect(promptSource).toMatch(/const morphShownAt = useFaceShownAt\(answersCard \? 'answer-control' : morphMode\);/);
    expect(promptSource).toMatch(/const answerShownAt = useFaceShownAt\(answerMode \?\? 'none'\);/);
  });

  it('wires the cancel path to both Cancel faces', () => {
    const loneCancel = promptSource.match(/answerMode === 'cancel' \?[\s\S]*?<\/button>/)?.[0] ?? '';
    for (const [face, source] of [['morphActivate', findMorphButton()], ['answerCancelActivate', loneCancel]]) {
      for (const handler of ['onTouchStart', 'onTouchMove', 'onTouchCancel']) {
        expect(source).toMatch(new RegExp(`${handler}=\\{${face}\\.${handler}\\}`));
      }
    }
  });

  it('gives the lone answer Submit a touch path', () => {
    expect(promptSource).toMatch(/onTouchEnd=\{answerSubmitActivate\.onTouchEnd\}/);
  });

  it('never cancels mousedown on a face in this row', () => {
    for (const source of [promptSource, splitButtonSource, bannerSource]) {
      expect(source).not.toMatch(/onMouseDown=/);
      expect(source).not.toMatch(/\bholdFocusOnPress\b/);
    }
  });

  it('gives Diff a touch path, since it sits in the row and is not destructive', () => {
    expect(bannerSource).toMatch(/onTouchEnd=\{activate\.onTouchEnd\}/);
    expect(bannerSource).toMatch(/import\s*\{[^}]*\buseTouchActivated\b[^}]*\}/);
  });

  it('routes the multi-select Submit through the lone answer Submit touch path', () => {
    expect(findActivationBody('answerSubmitActivate')).toMatch(/if \(hasPendingMultiQ\) void submitMultiAnswer\(\);/);
  });

  it('enumerates every touch path in the row, and marks each destructive one', () => {
    expect((promptSource.match(/onTouchEnd=/g) ?? []).length).toBe(3);
    expect((bannerSource.match(/onTouchEnd=/g) ?? []).length).toBe(1);
    expect(findMorphButton()).toMatch(/onTouchEnd=\{morphActivate\.onTouchEnd\}/);
    const loneCancel = promptSource.match(/answerMode === 'cancel' \?[\s\S]*?<\/button>/);
    expect(loneCancel, 'the lone answer Cancel was not found').not.toBeNull();
    expect(loneCancel![0]).toMatch(/onTouchEnd=\{answerCancelActivate\.onTouchEnd\}/);
    expect(findActivationBody('morphActivate')).toMatch(/morphMode === 'cancel',\s*morphShownAt,\s*\)$/);
    expect(findActivationBody('answerCancelActivate')).toMatch(/true,\s*answerShownAt,\s*\)$/);
    expect(findActivationBody('answerSubmitActivate')).not.toMatch(/destructive/);
    expect(bannerSource).toMatch(/const activate = useTouchActivated\(\(\) => openThreadDiff\(threadId\)\);/);
  });

  it('leaves the change-action banner Apply on click', () => {
    expect(bannerSource).not.toMatch(/primaryTouchActivate/);
  });

  it('blurs from inside each touch-activated action', () => {
    const submitFn = promptSource.match(/async function submit\(\)[\s\S]*?\n  \}/);
    const multiFn = promptSource.match(/async function submitMultiAnswer\(\)[\s\S]*?\n  \}/);
    const diffFn = bannerSource.match(/function DiffButton\([\s\S]*?\n\}/);
    expect(submitFn, 'submit() not found').not.toBeNull();
    expect(multiFn, 'submitMultiAnswer() not found').not.toBeNull();
    expect(diffFn, 'DiffButton not found').not.toBeNull();
    const openDiffFn = bannerSource.match(/function openThreadDiff\([\s\S]*?\n\}/);
    expect(diffFn![0]).toMatch(/openThreadDiff\(threadId\)/);
    expect(openDiffFn, 'openThreadDiff not found').not.toBeNull();
    expect(openDiffFn![0]).toMatch(/blurPromptInputIfFocused\(\)/);
    expect(submitFn![0]).toMatch(/if \(isMobile\(\)\) el\?\.blur\(\)/);
    expect(multiFn![0]).toMatch(/if \(isMobile\(\)\) el\?\.blur\(\)/);
  });

  it('never returns from submit without saying something', () => {
    expect(promptSource).toMatch(/queueUploadSend\(threadId, \{ useCodingAgent, context \}\);/);
    expect(promptSource).toContain('uploadSendNotice(queuedUploadSends.value.get(focusedTid)');
    expect(promptSource).toContain('data-role="upload-send-notice"');
  });
});
