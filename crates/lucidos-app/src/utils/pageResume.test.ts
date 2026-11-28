import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const platform = vi.hoisted(() => ({ webkit: true }));
vi.mock('./platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./platform')>()),
  isWebKit: () => platform.webkit,
}));

import {
  reduceResumeGuard,
  isIrreversibleTapTarget,
  onPageResume,
  repaintLandedContent,
  IRREVERSIBLE_TAP_SELECTOR,
  type ResumeEvent,
} from './pageResume';
import { OPEN_REPAINT_BURST_DELAYS_MS } from './repaintBurst';

describe('reduceResumeGuard', () => {
  it('a foreground-resume arms the guard and requests a repaint', () => {
    expect(reduceResumeGuard(false, { kind: 'resume' })).toEqual({
      armed: true,
      effect: { swallow: false, repaint: true },
    });
  });

  it('while armed, a tap on an irreversible button is swallowed (and repaints + disarms)', () => {
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
    expect(feed({ kind: 'resume' })).toEqual({ swallow: false, repaint: true });
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: true, repaint: true });
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: false, repaint: false });
  });

  it('a benign first tap disarms without swallowing, so the next tap is honored', () => {
    const feed = driver();
    feed({ kind: 'resume' });
    expect(feed({ kind: 'click', irreversible: false })).toEqual({ swallow: false, repaint: true });
    expect(feed({ kind: 'click', irreversible: true })).toEqual({ swallow: false, repaint: false });
  });

  it('honors the FIRST deliberate tap once the content has repainted (the reported regression)', () => {
    const feed = driver();
    expect(feed({ kind: 'resume' })).toEqual({ swallow: false, repaint: true });
    expect(feed({ kind: 'settled' })).toEqual({ swallow: false, repaint: false });
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
    expect(resumeSrc).toMatch(/if\s*\(installed\s*\|\|\s*!isWebKit\(\)\)\s*return;/);
  });

  it('still arms the wake-tap swallow on iOS alone', () => {
    expect(resumeSrc).toMatch(/if\s*\(isIOS\(\)\)\s*document\.addEventListener\(\s*'click'/);
  });

  it('disarms after the post-resume repaint settles via a double requestAnimationFrame', () => {
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

describe('repaintLandedContent', () => {
  beforeEach(() => {
    platform.webkit = true;
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('fires every subscriber once per arrival delay', () => {
    const a = vi.fn();
    const b = vi.fn();
    const stopA = onPageResume(a);
    const stopB = onPageResume(b);

    repaintLandedContent();
    expect(a).toHaveBeenCalledTimes(1);
    vi.advanceTimersByTime(Math.max(...OPEN_REPAINT_BURST_DELAYS_MS));
    expect(a).toHaveBeenCalledTimes(OPEN_REPAINT_BURST_DELAYS_MS.length);
    expect(b).toHaveBeenCalledTimes(OPEN_REPAINT_BURST_DELAYS_MS.length);

    stopA();
    stopB();
  });

  it('spans the arrival of a lazily-chunked view, not just the next frame', () => {
    expect(Math.max(...OPEN_REPAINT_BURST_DELAYS_MS)).toBeGreaterThanOrEqual(1000);
  });

  it('costs nothing off WebKit', () => {
    platform.webkit = false;
    const cb = vi.fn();
    const stop = onPageResume(cb);

    repaintLandedContent();
    vi.advanceTimersByTime(Math.max(...OPEN_REPAINT_BURST_DELAYS_MS));
    expect(cb).not.toHaveBeenCalled();

    stop();
  });

  it('never arms the wake-tap swallow', () => {
    const landed = resumeSrc.match(/export function repaintLandedContent[\s\S]*?\n}/)?.[0] ?? '';
    expect(landed).toMatch(/fireRepaint/);
    expect(landed).not.toMatch(/step\(|'resume'|scheduleSettle/);
  });
});
