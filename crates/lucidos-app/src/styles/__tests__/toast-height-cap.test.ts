/**
 * A toast is a SUMMARY with somewhere to go, so it may never grow into a
 * document viewer.
 *
 * The regression (2026-08-09, iOS PWA at 390pt): a Mac-memory-watchdog
 * notification, whose body embeds a `ps` command and a full metrics dump,
 * rendered as a card running from under the app header down to the composer,
 * roughly 80% of the screen. `.toast` was capped at `100dvh - 6rem`, which is a
 * cap in name only, and desktop carried the identical rule. The fix is an
 * ABSOLUTE cap with the viewport term demoted to a ceiling under it, plus the
 * scroll-inside behaviour that was already there.
 *
 * Scanned rather than measured in a browser, because the failure is a property
 * of the rule and not of any one device: a viewport-only cap looks fine at every
 * size until the body is long enough, and reproducing it needs both a long
 * message and a short screen. `rulesTargeting` (not `block`) is the reader on
 * purpose, so a `@media` copy or a compound selector quietly raising the cap
 * back is caught rather than passed over by a first textual match.
 *
 * The cap is only half the contract: capping a toast that CANNOT scroll just
 * clips it, which is the second bug in this file (see the spinner test at the
 * bottom). The two are tested together because they are one promise.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../mobile.css'), 'utf-8');

/** Ceiling for the absolute cap, in rem. Not the value itself: the point is
 *  that a toast stays a toast, so the test pins the ORDER OF MAGNITUDE and
 *  leaves the exact figure free to be tuned. Anything at or under this leaves
 *  most of a phone screen showing the thread behind it. */
const ABSOLUTE_CAP_CEILING_REM = 16;

/** The absolute leading term of a `max-height`, in rem, or null when the value
 *  carries none. One reader for both tests below, so "has an absolute cap" and
 *  "the cap is small enough" can never be judged by two different rules: a
 *  guard that accepted any rem at all would wave through
 *  `min(60rem, calc(…))`, which reinstates exactly the bug this file exists to
 *  prevent. */
function absoluteCapRem(maxHeight: string | null): number | null {
  const m = maxHeight?.match(/(\d+(?:\.\d+)?)rem\s*,/);
  return m ? Number(m[1]) : null;
}

/** Does this `max-height` keep a toast toast-sized? */
function isCapped(maxHeight: string | null): boolean {
  const rem = absoluteCapRem(maxHeight);
  return rem !== null && rem <= ABSOLUTE_CAP_CEILING_REM;
}

describe('a long toast body cannot swallow the viewport', () => {
  it('caps .toast at an absolute height, not only a viewport-derived one', () => {
    const maxHeight = decl(block(componentsCss, '.toast {'), 'max-height');
    expect(maxHeight).not.toBeNull();

    // `min(<absolute>, <viewport ceiling>)`. A bare `calc(100dvh - …)` is the
    // bug: on a tall screen it permits a near-full-height card.
    expect(maxHeight!.startsWith('min(')).toBe(true);

    expect(absoluteCapRem(maxHeight), `no absolute rem cap in "${maxHeight}"`).not.toBeNull();
    expect(absoluteCapRem(maxHeight)!).toBeLessThanOrEqual(ABSOLUTE_CAP_CEILING_REM);

    // And still never taller than the VISIBLE app height: --app-height is what
    // keeps iOS honest once the keyboard pushes the visual viewport up, so the
    // absolute cap must not replace that term, only lead it.
    expect(maxHeight).toContain('--app-height');
  });

  it('is capped everywhere, with no sheet raising it back', () => {
    const raisers = [...rulesTargeting(componentsCss, 'toast'), ...rulesTargeting(mobileCss, 'toast')]
      .filter((rule) => rule.props.has('max-height'))
      .filter((rule) => !isCapped(rule.props.get('max-height')!));

    expect(
      raisers.map((r) => `${r.atRules} ${r.selector} { max-height: ${r.props.get('max-height')} }`),
      'a later rule re-raises the toast cap past the absolute one',
    ).toEqual([]);
  });

  it('scrolls the overflow inside the message, leaving [Open] and the X reachable', () => {
    const message = block(componentsCss, '.toast-message {');
    expect(decl(message, 'overflow-y')).toBe('auto');
    // Without these the flex item takes its content height and overflows the
    // capped body, so the cap would clip the message instead of scrolling it:
    // `min-height: 0` lets the box shrink, `align-self: stretch` overrides
    // .toast-body's `align-items: flex-start` so it takes the shrunk height.
    expect(decl(message, 'min-height')).toBe('0');
    expect(decl(message, 'align-self')).toBe('stretch');
    // The body must be able to shrink below its content in .toast's column, or
    // there is no capped height for the message to stretch into.
    expect(decl(block(componentsCss, '.toast-body {'), 'min-height')).toBe('0');
    // The actions row and the absolutely-positioned close X are siblings of the
    // scrolling message, not inside it, so a clamped toast still offers its
    // action without the user scrolling to find it.
    const toast = block(componentsCss, '.toast {');
    expect(decl(toast, 'display')).toBe('flex');
    expect(decl(toast, 'flex-direction')).toBe('column');
  });

  /**
   * The scroll container may not be an ancestor of the mini-spinner.
   *
   * A scroll container clips its own painted overflow on both axes, and the
   * spinner rotates via `transform`, so at 45° it paints ~√2 outside its square
   * layout box and gets sheared. When `.toast-body` was the scroller that was
   * patched with `.toast-body:has(.mini-spinner) { overflow: visible }`, written
   * when a spinning toast was always one line. It then switched the scroll off
   * for EVERY spinning toast, so the build toast's commit list was clipped at
   * the cap with no way to reach the rest (reported 2026-08-11).
   *
   * Scanned rather than measured, for the same reason as the cap above: the
   * failure is a property of the rule, and reproducing it needs a spinning
   * toast whose body happens to be long.
   */
  it('does not let a spinner switch the toast scroll off', () => {
    for (const sheet of [componentsCss, mobileCss]) {
      const offenders = [...rulesTargeting(sheet, 'toast-body'), ...rulesTargeting(sheet, 'toast')]
        .filter((rule) => ['overflow', 'overflow-y'].some((p) => rule.props.has(p)))
        .filter((rule) => rule.props.get('overflow') !== 'hidden');

      expect(
        offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
        'an ancestor of .toast-message owns a scroll the spinner would then have to switch off',
      ).toEqual([]);
    }
  });
});
