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
    // Everything after line 1. This is the box the cap makes scroll.
    const sections = block(componentsCss, '.toast-sections {');
    expect(decl(sections, 'overflow-y')).toBe('auto');
    // Without this the flex item takes its content height and overflows the
    // capped column, so the cap would clip the sections instead of scrolling
    // them: `min-height: 0` is what lets the box shrink to the room it has.
    expect(decl(sections, 'min-height')).toBe('0');
    // Line 1 scrolls too, for the message that is ALL line 1: a section-less
    // body has nothing in the box above, so a long one would be clipped.
    const heading = block(componentsCss, '.toast-heading {');
    expect(decl(heading, 'overflow-y')).toBe('auto');
    // The column must be able to shrink below its content inside .toast, or
    // there is no capped height for either box to scroll in.
    expect(decl(block(componentsCss, '.toast-body {'), 'min-height')).toBe('0');
    // The actions row and the absolutely-positioned close X are siblings of the
    // scrolling column, not inside it, so a clamped toast still offers its
    // action without the user scrolling to find it.
    const toast = block(componentsCss, '.toast {');
    expect(decl(toast, 'display')).toBe('flex');
    expect(decl(toast, 'flex-direction')).toBe('column');
  });

  /**
   * The scroll box starts BELOW the close X, so no scrollbar runs under it.
   *
   * A scrollbar is drawn at its own box's right edge. The close X owns the
   * top-right corner, so a scroll box reaching up beside it would put the
   * scrollbar under the glyph. The heading is what holds them apart: it is at
   * least as tall as the button, and reserves as much on the right.
   *
   * One token carries all three, which is the assertion. A literal restated
   * here would keep passing while the button grew out from under it.
   */
  it('floors the heading at the close button, and sizes the button from it', () => {
    const heading = block(componentsCss, '.toast-heading {');
    expect(decl(heading, 'min-height')).toBe('var(--toast-close-size)');
    expect(decl(heading, 'margin-right')).toBe('var(--toast-close-size)');

    const close = block(componentsCss, '.toast-close {');
    expect(decl(close, 'width')).toBe('var(--toast-close-size)');
    expect(decl(close, 'height')).toBe('var(--toast-close-size)');
  });

  /**
   * The scroll box reserves nothing on its right, so its scrollbar lands in the
   * card's right rail instead of inside the text column.
   *
   * That was the reported look. The scroll lived in a box the close X's gutter
   * had already narrowed, so the bar sat mid-card with the X beyond it. Its
   * `--bg-primary` track then read as a black slot cut through the message.
   */
  it('runs the scroll box to the toast content edge', () => {
    for (const sheet of [componentsCss, mobileCss]) {
      const offenders = rulesTargeting(sheet, 'toast-sections')
        .filter((rule) =>
          ['padding-right', 'margin-right', 'padding', 'margin', 'border-right', 'width']
            .some((p) => rule.props.has(p)),
        );

      expect(
        offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
        'a right-hand reserve pulls the scrollbar back inside the text column',
      ).toEqual([]);
    }
  });

  /**
   * No scroll container may be an ancestor of the mini-spinner.
   *
   * A scroll container clips its own painted overflow on both axes, and the
   * spinner rotates via `transform`, so at 45° it paints ~√2 outside its square
   * layout box and gets sheared. When `.toast-body` was the scroller that was
   * patched with `.toast-body:has(.mini-spinner) { overflow: visible }`, written
   * when a spinning toast was always one line. It then switched the scroll off
   * for EVERY spinning toast, so the build toast's commit list was clipped at
   * the cap with no way to reach the rest.
   *
   * The icon is out of the flow now, which settles it structurally: it is a
   * child of `.toast`, and `.toast` clips rather than scrolls. Both halves are
   * asserted, since an icon put back in the flow would land in a scroll box.
   */
  it('keeps the spinner out of every scroll box', () => {
    const icon = block(componentsCss, '.toast-icon {');
    expect(decl(icon, 'position')).toBe('absolute');
    // Over the gutter .toast-body pads out for it, so a clickable toast has no
    // dead spot where the icon covers the message.
    expect(decl(icon, 'pointer-events')).toBe('none');

    for (const sheet of [componentsCss, mobileCss]) {
      const offenders = [...rulesTargeting(sheet, 'toast-body'), ...rulesTargeting(sheet, 'toast')]
        .filter((rule) => ['overflow', 'overflow-y'].some((p) => rule.props.has(p)))
        .filter((rule) => rule.props.get('overflow') !== 'hidden');

      expect(
        offenders.map((r) => `${r.atRules} ${r.selector} { ${r.body} }`),
        'an ancestor of the icon owns a scroll the spinner would then have to switch off',
      ).toEqual([]);
    }
  });
});
