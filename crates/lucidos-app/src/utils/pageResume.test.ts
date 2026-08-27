import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';
import {
  reduceResumeGuard,
  isIrreversibleTapTarget,
  IRREVERSIBLE_TAP_SELECTOR,
  type ResumeEvent,
} from './pageResume';

describe('reduceResumeGuard', () => {
  it('a foreground-resume arms the guard and requests a repaint', () => {
    expect(reduceResumeGuard(false, { kind: 'resume' })).toEqual({
      armed: true,
      effect: { swallow: false, repaint: true },
    });
  });

  it('while armed, a tap on an irreversible button is swallowed (and repaints + disarms)', () => {
    // The reported bug: the tap that wakes a blanked iOS layer must NOT answer
    // the invisible question / grant the invisible permission underneath it.
    expect(reduceResumeGuard(true, { kind: 'click', irreversible: true })).toEqual({
      armed: false,
      effect: { swallow: true, repaint: true },
    });
  });

  it('while armed, a tap elsewhere disarms + repaints but is not swallowed', () => {
    expect(reduceResumeGuard(true, { kind: 'click', irreversible: false })).toEqual({
      armed: false,
      effect: { swallow: false, repaint: true },
    });
  });

  it('a tap with no preceding resume is honored (never swallowed)', () => {
    expect(reduceResumeGuard(false, { kind: 'click', irreversible: true })).toEqual({
      armed: false,
      effect: { swallow: false, repaint: false },
    });
  });

  it('settling (post-resume repaint complete) disarms without swallowing or repainting', () => {
    // Once the page has painted the post-resume frames the content is visible
    // again, so the wake-tap window is over: disarm so the user's first real tap
    // is honored. No further repaint needed — the proactive one already ran.
    expect(reduceResumeGuard(true, { kind: 'settled' })).toEqual({
      armed: false,
      effect: { swallow: false, repaint: false },
    });
  });

  it('settling while not armed is a no-op', () => {
    expect(reduceResumeGuard(false, { kind: 'settled' })).toEqual({
      armed: false,
      effect: { swallow: false, repaint: false },
    });
  });
});

describe('resume → wake-tap → deliberate-tap sequence (the reported bug)', () => {
  function driver() {
    let armed = false;
    return (ev: ResumeEvent) => {
      const r = reduceResumeGuard(armed, ev);
      armed = r.armed;
      return r.effect;
    };
  }

  it('swallows the first post-resume tap on a question, then honors the next', () => {
    const feed = driver();
    // PWA returns from background — content layer may be blanked (black).
    expect(feed({ kind: 'resume' })).toEqual({ swallow: false, repaint: true });
    // User taps the (invisible) option to bring the screen back — must not answer.
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: true, repaint: true });
    // Screen is visible now; the user taps the option deliberately — it answers.
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: false, repaint: false });
  });

  it('a benign first tap disarms without swallowing, so the next tap is honored', () => {
    const feed = driver();
    feed({ kind: 'resume' });
    // First interaction lands somewhere harmless (e.g. the prompt) — disarms.
    expect(feed({ kind: 'click', irreversible: false })).toEqual({ swallow: false, repaint: true });
    // The subsequent tap on a question option answers normally.
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: false, repaint: false });
  });

  it('honors the FIRST deliberate tap once the content has repainted (the reported regression)', () => {
    const feed = driver();
    // PWA returns; the proactive repaint un-blanks the layer.
    expect(feed({ kind: 'resume' })).toEqual({ swallow: false, repaint: true });
    // The double-rAF repaint completes — content is visible before the user moves
    // to tap (human reaction time >> two frames).
    expect(feed({ kind: 'settled' })).toEqual({ swallow: false, repaint: false });
    // The user's FIRST tap on the now-visible question must register, not be
    // eaten as a wake-tap. This is the bug: previously this tap was swallowed.
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: false, repaint: false });
  });
});

describe('isIrreversibleTapTarget', () => {
  const fake = (matches: boolean) => ({
    closest: (sel: string) => (matches && sel === IRREVERSIBLE_TAP_SELECTOR ? {} : null),
  });

  it('matches an element whose closest() resolves a question/permission button', () => {
    expect(isIrreversibleTapTarget(fake(true) as unknown as EventTarget)).toBe(true);
  });

  it('does not match an element outside the irreversible families', () => {
    expect(isIrreversibleTapTarget(fake(false) as unknown as EventTarget)).toBe(false);
  });

  it('is null-safe and ignores targets without closest()', () => {
    expect(isIrreversibleTapTarget(null)).toBe(false);
    expect(isIrreversibleTapTarget({} as unknown as EventTarget)).toBe(false);
  });

  it('targets the live question-option and permission-action buttons', () => {
    expect(IRREVERSIBLE_TAP_SELECTOR).toContain('.question-option');
    expect(IRREVERSIBLE_TAP_SELECTOR).toContain('.permission-actions button');
  });
});

// Source-wiring guards (mirrors prompt-cancel-tap-gate.test.ts): the behavior
// lives in capture-phase DOM listeners that the minimal (non-jsdom) test
// environment can't dispatch through, so pin the wiring against regressions.
const here: string = dirname(fileURLToPath(import.meta.url));
const resumeSrc = readFileSync(resolve(here, './pageResume.ts'), 'utf-8');
const threadViewSrc = readFileSync(resolve(here, '../components/chat/ThreadView.tsx'), 'utf-8');

describe('pageResume DOM wiring', () => {
  it('listens to all three foreground-resume signals, not just visibilitychange', () => {
    expect(resumeSrc).toMatch(/'visibilitychange'/);
    expect(resumeSrc).toMatch(/'pageshow'/);
    expect(resumeSrc).toMatch(/'focus'/);
  });

  it('swallows the wake-tap via a capture-phase click listener', () => {
    expect(resumeSrc).toMatch(/addEventListener\(\s*'click'[\s\S]*?,\s*true\s*\)/);
  });

  it('installs on any WebKit client, so the packaged desktop app repaints too', () => {
    // The compositor bug belongs to the engine. Gating the install on iOS left
    // the Mac app resuming onto a blank transcript with no lever to recover it.
    expect(resumeSrc).toMatch(/if\s*\(installed\s*\|\|\s*!isWebKit\(\)\)\s*return;/);
  });

  it('still arms the wake-tap swallow on iOS alone', () => {
    // A desktop window is RAISED by a click, so swallowing the first one there
    // would eat an ordinary click-to-focus. Only the phone taps a blank layer.
    expect(resumeSrc).toMatch(/if\s*\(isIOS\(\)\)\s*document\.addEventListener\(\s*'click'/);
  });

  it('disarms after the post-resume repaint settles via a double requestAnimationFrame', () => {
    // The wake-tap swallow must only bite while the layer might still be black.
    // Once the page paints the post-resume frames, the guard disarms so a
    // deliberate first tap is honored — pinned so a refactor can't drop it back
    // to the unconditional first-tap swallow.
    expect(resumeSrc).toMatch(/requestAnimationFrame\([\s\S]*?requestAnimationFrame/);
    expect(resumeSrc).toMatch(/kind:\s*'settled'/);
  });
});

describe('ThreadView resume repaint wiring', () => {
  it('drives its WebKit repaint off the shared onPageResume signal', () => {
    expect(threadViewSrc).toMatch(/import\s*\{[^}]*\bonPageResume\b[^}]*\}\s*from\s*['"]\.\.\/\.\.\/utils\/pageResume['"]/);
    expect(threadViewSrc).toMatch(/onPageResume\(/);
  });
});
