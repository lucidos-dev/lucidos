/**
 * Source scans over the one rule that puts a badge's number in the middle.
 *
 * The reported bug: every badge centred its text LINE BOX. A line box is not
 * the digits, because the baseline sits wherever the font's ascent and descent
 * put it, and those are asymmetric. Measured over the whole set, the digits rode
 * high by up to 1.55px in Chromium and 0.48px in WebKit. The same badge was
 * drawn differently on iOS and on Chrome.
 *
 * Across the badge there was a second one. `letter-spacing` is added after
 * every character, the last one included. Centring a box that carries that
 * trailing gap parks the digits half a gap left. The collapsed-section count
 * inherits its title's tracking, so it sat 0.65px left at 200% ui-scale.
 *
 * `styles/badges.css` fixes both in one place. What the fix depends on is not
 * visible from any one badge's own stylesheet, so these are the scans:
 *
 *  - the trim reaches the text, which needs the badge to stop being a flex box;
 *  - no badge inherits the tracking that leans it left;
 *  - the rules cover every badge that carries a number or a sign;
 *  - badges.css is imported LAST, which lets it win their `display`;
 *  - the two badges sized by that line box keep their height.
 *
 * Nothing renders in a unit test, so the numbers above are evidence rather
 * than an assertion. What is asserted here is the shape that produced them.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, selectorList, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const badgesCss: string = readFileSync(resolve(stylesDir, 'badges.css'), 'utf-8');
const mainTsx: string = readFileSync(resolve(stylesDir, '..', 'main.tsx'), 'utf-8');

/** Every badge whose content is a number or a sign, so every badge that has a
 *  glyph to centre. A badge drawing only a dot is deliberately absent: it has
 *  no text, and the trim would be a no-op on it. */
const GLYPH_BADGES = [
  'badge',                          // header counts, the "!" state badge, the unread count
  'drawer-view-count',              // the thread filter's per-view counts
  'collapse-count-badge',           // a collapsed drawer section, and a collapsed family
  'drawer-badge',                   // the menu drawer's changes count
  'brand-menu-ws-badge',            // unread per workspace, in the menu and the switcher
  'ws-picker-badge',                // unread per workspace, in the gateway picker
  'thread-status-question-badge',   // the "?" on a thread waiting for an answer
];

const shared = cssRules(badgesCss).find(r => r.props.get('text-box-trim') === 'trim-both');
const tracking = cssRules(badgesCss).find(r => r.props.get('letter-spacing') === 'normal');

describe('one rule centres the glyph in every badge that has one', () => {
  it('trims the line box to the cap band, and centres THAT', () => {
    // `text-box-trim` takes the font metrics out of the question. The line box
    // becomes the cap band, so the box being centred IS the digits.
    expect(shared, 'no trim rule in badges.css').toBeDefined();
    expect(shared!.props.get('text-box-edge')).toBe('cap alphabetic');
    expect(shared!.props.get('align-content')).toBe('center');
    expect(shared!.props.get('text-align')).toBe('center');
  });

  it('stops each badge being a flex box, which is what lets the trim land', () => {
    // The trim applies to the block container holding the text. A flex
    // container's text lives in an ANONYMOUS item, which inherits none of it.
    // So the trim is silently a no-op while the badge stays `display: flex`.
    // `inline-block` also survives a badge landing in a plain line of text.
    expect(shared!.props.get('display')).toBe('inline-block');
  });

  it('is gated on support, so an older browser keeps the flex centring', () => {
    // There is no fallback to write. Ungated, a browser with `align-content`
    // and no trim would centre the line box the flex rules already centre.
    // One with neither would top-align the digits.
    expect(shared!.atRules).toBe('@supports (text-box-trim: trim-both)');
  });

  it('names every badge that carries a number or a sign', () => {
    expect(selectorList(shared!.selector)).toEqual(GLYPH_BADGES.map(c => `.${c}`));
  });

  it('takes no tracking from its host, and takes it UNGATED', () => {
    // A trailing letter-space with no glyph after it is dead width on the
    // right, so centring the box leans the digits left. Dropping the tracking
    // removes the gap; a count is data rather than a heading. Ungated, because
    // this half needs no new CSS feature.
    expect(tracking, 'no letter-spacing rule in badges.css').toBeDefined();
    expect(tracking!.atRules, 'the lean is there with or without the trim').toBe('');
    expect(selectorList(tracking!.selector)).toEqual(GLYPH_BADGES.map(c => `.${c}`));
  });

  it('keeps the height of the two badges the trim would shrink', () => {
    // These two took their height from the line box, so trimming it would cost
    // them a few pixels of pill. `1lh` IS that line box, stated. Every other
    // badge already declares a `height` or a `min-height` of its own.
    const held = cssRules(badgesCss).filter(r => r.props.get('min-height') === '1lh');
    expect(held).toHaveLength(1);
    expect(selectorList(held[0].selector))
      .toEqual(['.collapse-count-badge', '.brand-menu-ws-badge']);
  });

  it('leaves the one badge whose content is not text on flex centring', () => {
    // While the engine builds, the brand badge holds a spinning glyph instead
    // of the "!". A line box trimmed to a cap band it has no part in would park
    // that glyph off centre.
    const spinner = cssRules(badgesCss)
      .find(r => r.selector === '.badge:has(.brand-badge-spinner)');
    expect(spinner?.props.get('display')).toBe('flex');
    expect(spinner?.props.get('align-items')).toBe('center');
    expect(spinner?.props.get('text-box-trim')).toBe('none');
  });
});

describe('nothing outranks it', () => {
  it('is the LAST stylesheet main.tsx imports', () => {
    // Its selectors are single classes, exactly like the rules it re-homes, so
    // source order is the whole of why it wins their `display`. An import added
    // below it silently hands the badges back their flex centring.
    const imports: string[] = [...mainTsx.matchAll(/^import '(\.\/styles\/[^']+)';$/gm)]
      .map(m => m[1]);
    expect(imports.length, 'no style imports found').toBeGreaterThan(1);
    expect(imports[imports.length - 1]).toBe('./styles/badges.css');
  });

  it('is the only place a badge is given a display it cannot beat', () => {
    // Two ways to outrank it, so two conditions.
    //
    // A heavier selector beats it outright, wherever it lives, and
    // `.app-header .badge` is the shape to expect. A single class only ties, so
    // order decides, and order is only decided for the sheets main.tsx imports.
    // A component keeps its own sheet beside itself
    // (`components/search/SearchEverywhere.css`), and Vite injects that with the
    // chunk, which lands AFTER badges.css. So out there even a tie wins.
    //
    // The whole of `src/` for that reason, not `src/styles`, which is the same
    // sweep and the same reasoning as `header-badge-ring.test.ts`.
    const offenders: string[] = [];
    let seen = 0;
    for (const path of styleSheetPaths(resolve(stylesDir, '..'))) {
      if (path.endsWith('badges.css')) continue;
      const ordered: boolean = path.includes('/styles/');
      const css: string = readFileSync(path, 'utf-8');
      for (const cls of GLYPH_BADGES) {
        for (const rule of rulesTargeting(css, cls)) {
          if (!rule.props.has('display')) continue;
          seen++;
          const beats = selectorList(rule.selector)
            .filter(one => one.includes(cls) && (!ordered || one !== `.${cls}`));
          if (beats.length) offenders.push(`${path.split('/src/')[1]}: ${beats.join(', ')}`);
        }
      }
    }
    // A FLOOR, so an empty sweep cannot read as a clean one. Five sheets set a
    // badge's own `display` today, which is what badges.css re-homes.
    expect(seen, 'the sweep found no badge display rules at all').toBeGreaterThanOrEqual(5);
    expect(offenders).toEqual([]);
  });
});

describe('a drawer section and its count share one cap band', () => {
  const drawerCss: string = readFileSync(resolve(stylesDir, 'drawer.css'), 'utf-8');
  const gated = cssRules(drawerCss).filter(r => r.atRules === '@supports (text-box-trim: trim-both)');

  it('gives the label the same trim the count now has', () => {
    // The count sits at its pill's middle once trimmed. The label's caps ride
    // above its own line box's middle until it is trimmed too, and the flex row
    // centres both boxes on one axis. Trimming both is what puts the two cap
    // bands on the same line at any font and any ui-scale.
    const label = gated.find(r => r.selector === '.drawer-section-label');
    expect(label?.props.get('text-box-trim')).toBe('trim-both');
    expect(label?.props.get('text-box-edge')).toBe('cap alphabetic');
    expect(label?.props.get('align-content')).toBe('center');
  });

  it('holds the label at its untrimmed box, which two other things ride on', () => {
    // The trim shrinks the CONTENT box, and `1lh` puts the element's own box
    // back. So the row is the same height collapsed and expanded, and the
    // shimmer still has the whole glyph to paint over.
    const label = gated.find(r => r.selector === '.drawer-section-label');
    expect(label?.props.get('min-height')).toBe('1lh');
  });

  it('retires the hand-tuned lift wherever the trim lands, and only there', () => {
    // The nudge stood in for the gap the trim now closes. It stays outside the
    // gate, because a browser without the trim still has the gap to correct.
    const retired = gated.find(r =>
      r.selector === '.list-section-title-collapsible > .collapse-count-badge');
    expect(retired?.props.get('top')).toBe('0');
    const base = cssRules(drawerCss).find(r =>
      r.selector === '.list-section-title-collapsible > .collapse-count-badge'
      && r.atRules === '');
    expect(base?.props.get('top'), 'the untrimmed fallback keeps its lift').toBe('-0.0625rem');
  });
});
