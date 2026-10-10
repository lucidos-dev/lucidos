/**
 * A scroll container must never be made `pointer-events: none`.
 *
 * The two look like the same knob and are not. A touch is hit-tested to the
 * deepest element that answers the pointer, and the browser then pans that
 * element's nearest scrollable ancestor. Take the scroller itself out of
 * hit-testing and the touch resolves to whatever is BEHIND it, which scrolls
 * nothing, so the surface stops scrolling entirely. Blocking its CHILDREN
 * instead reaches the same "no stray taps" end (`pointer-events` is inherited,
 * so the whole subtree goes inert) while the pan still lands.
 *
 * The regression this pins: `:root[data-keyboard-active] .thread-content` was
 * `pointer-events: none`, so focusing the composer on a phone froze the
 * transcript. With a live multi-select question card that is the whole
 * interaction: the card is in the transcript, the Submit is in the prompt row,
 * and the reader could neither scroll to the options nor tap them. The
 * horizontal pane swipe kept working, because its handler sits on the swipe
 * container above the transcript, which is exactly how it was reported: "swipe
 * up/down did not work, frozen, swipe left/right worked".
 *
 * A source scan rather than a browser test, for the same reason as the sibling
 * guards: the regression is about which declaration is written, and the rule is
 * behind a state (`data-keyboard-active`) plus a viewport that only the mobile
 * e2e projects reach. `e2e/multiselect-transcript-scroll-mobile.spec.ts` covers
 * the behaviour end to end; this fails the moment the declaration comes back,
 * on any scroller, in any sheet.
 *
 * The OTHER half of that rule (the block must still exist, on the children, with
 * the compose-empty carve-out after it) is owned by
 * `components/chat/__tests__/welcome-keyboard-active.test.ts`, so it is not
 * restated here.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve, relative } from 'node:path';

import { rulesTargeting, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const STYLES_ROOT: string = resolve(here, '..');

/** The scroll containers a finger pans directly. Each owns `overflow-y` and is
 *  the element the browser scrolls, so each is disqualified from ever being
 *  taken out of hit-testing. Add a class here when a new pannable surface is
 *  introduced. */
const SCROLLERS = [
  'thread-content',        // the conversation transcript
  'content-pane-body',     // the canvas pane's scrolling body
  'thread-drawer-list',    // the thread drawer's list
] as const;

describe('scroll containers stay hit targets', () => {
  const offenders: string[] = [];
  for (const path of styleSheetPaths(STYLES_ROOT)) {
    const css: string = readFileSync(path, 'utf8');
    for (const scroller of SCROLLERS) {
      for (const rule of rulesTargeting(css, scroller)) {
        if (rule.props.get('pointer-events') !== 'none') continue;
        offenders.push(`${relative(STYLES_ROOT, path)}: ${rule.atRules} { ${rule.selector} }`);
      }
    }
  }

  it('never sets pointer-events: none on the scroller itself', () => {
    expect(
      offenders,
      'A scroller taken out of hit-testing cannot be panned: the touch resolves to whatever '
      + 'sits behind it, which scrolls nothing. To block stray taps, target the children '
      + '(`.scroller > *`) instead. `pointer-events` inherits, so the subtree still goes inert '
      + 'while the pan keeps landing on the scroller.',
    ).toEqual([]);
  });
});
