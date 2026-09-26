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
 * trailing gap parks the digits half a gap left. The section count
 * inherits its title's tracking, so it sat 0.65px left at 200% ui-scale.
 *
 * Then a third. Chrome paints a baseline on a whole CSS pixel, and a pill's
 * edges too. So the digit jumped a full pixel against its pill as the badge
 * moved by a fraction of one. The mark's unread count sat 0.5px low.
 *
 * `styles/badges.css` fixes all three in one place. What the fix depends on is
 * not visible from any one badge's own stylesheet, so these are the scans:
 *
 *  - the trim reaches the text, which needs the badge to stop being a flex box;
 *  - no badge inherits the tracking that leans it left;
 *  - the rules cover every badge that carries a number or a sign;
 *  - the pill height and the baseline's offset are whole pixels;
 *  - badges.css is imported LAST, which lets it win their `display` and box;
 *  - no badge hands its font to a pseudo-element, which the trim cannot see.
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
  'section-count-badge',            // a drawer section's thread count
  'drawer-badge',                   // the menu drawer's changes count
  'brand-menu-ws-badge',            // unread per workspace, in the menu and the switcher
  'ws-picker-badge',                // unread per workspace, in the gateway picker
  'thread-status-question-badge',   // the "?" on a thread waiting for an answer
];

/** The two badges that wear `.badge` and draw only a dot. No glyph, so no cap
 *  band to centre, and each keeps the size its own rule states. */
const GLYPHLESS = ['brand-badge-dot', 'system-attention-badge-corner'];

/** `.badge` minus the dots, at the specificity of `.badge` alone: `:where()`
 *  weighs nothing, so the `:not()` around it weighs nothing either. */
const SHARED_SELECTORS = GLYPH_BADGES.map(c => c === 'badge'
  ? `.badge:not(:where(${GLYPHLESS.map(g => `.${g}`).join(', ')}))`
  : `.${c}`);

/** Every property the shared rule decides, and so every one a heavier rule
 *  elsewhere could quietly take back. */
const OWNED = ['display', 'align-content', 'height', 'min-height', 'padding',
  'padding-top', 'padding-bottom', 'padding-block'];

const shared = cssRules(badgesCss).find(r => r.props.get('text-box-trim') === 'trim-both');
const tracking = cssRules(badgesCss).find(r => r.props.get('letter-spacing') === 'normal');

describe('one rule centres the glyph in every badge that has one', () => {
  it('trims the line box to the cap band, and parks it on a whole-pixel baseline', () => {
    // `text-box-trim` takes the font metrics out of the question. The line box
    // becomes the cap band, so its bottom edge IS the baseline. Aligned to the
    // END, the baseline sits exactly one bottom padding above the pill's edge.
    expect(shared, 'no trim rule in badges.css').toBeDefined();
    expect(shared!.props.get('text-box-edge')).toBe('cap alphabetic');
    expect(shared!.props.get('align-content')).toBe('end');
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

  it('names every badge that carries a number or a sign, and no dot', () => {
    expect(selectorList(shared!.selector)).toEqual(SHARED_SELECTORS);
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

  it('leaves the one badge whose content is not text on flex centring', () => {
    // While the engine builds, the brand badge holds a spinning glyph instead
    // of the "!". A line box trimmed to a cap band it has no part in would park
    // that glyph off centre, and so would the padding that lifts a baseline.
    const spinner = cssRules(badgesCss)
      .find(r => r.selector === '.badge:has(.brand-badge-spinner)');
    expect(spinner?.props.get('display')).toBe('flex');
    expect(spinner?.props.get('align-items')).toBe('center');
    expect(spinner?.props.get('text-box-trim')).toBe('none');
    expect(spinner?.props.get('padding-block')).toBe('0');
  });
});

describe('the pill and the baseline sit on whole pixels', () => {
  // Chrome paints a baseline on a whole CSS pixel and a pill's edges on whole
  // pixels, each rounding on its own. Make the pill's height and the
  // baseline's offset whole, and the two round together. The digit then stops
  // moving against its pill as the badge moves.

  it('rounds the design height to the whole pixel that splits most evenly', () => {
    // `1cap + 2 * round(...)` is the height whose leftover splits exactly
    // around the cap band, aimed between the two whole pixels at or below the
    // design. Rounding THAT, and clamping to those two, leaves the digit at
    // most a quarter pixel off its middle.
    expect(shared!.props.get('--badge-floor')).toBe('round(down, var(--badge-height), 1px)');
    expect(shared!.props.get('--badge-box')).toBe('clamp(var(--badge-floor) - 1px, '
      + 'round(1cap + 2 * round((var(--badge-floor) - 0.5px - 1cap) / 2, 1px), 1px), '
      + 'var(--badge-floor))');
  });

  it('never makes a pill taller than its design', () => {
    // The section count is exactly as tall as the header's line. One pixel
    // more grows the row and puts the label off the pixel grid, which
    // `.list-section-title-collapsible` in drawer.css records.
    const box = shared!.props.get('--badge-box') ?? '';
    expect(box.endsWith(', var(--badge-floor))'), 'the clamp caps the box at the design').toBe(true);
    expect(shared!.props.get('height')).toBe('var(--badge-box)');
    // A badge's own `min-height` is its design height, which can be the taller
    // of the two by up to a pixel. It must not undo the rounding.
    expect(shared!.props.get('min-height')).toBe('0');
  });

  it('lifts the baseline by a whole-pixel bottom padding', () => {
    // Named, because the section count scales from exactly this baseline.
    expect(shared!.props.get('--badge-baseline-inset'))
      .toBe('round((var(--badge-box) - 1cap) / 2, 1px)');
    expect(shared!.props.get('padding-block')).toBe('0 var(--badge-baseline-inset)');
  });

  it('has every badge with a pill state the height it was designed at', () => {
    // The plain per-view count draws no pill, so it has no height to round.
    // Its blue sibling is a `.badge` and inherits one.
    const missing: string[] = [];
    const sheets: string[] = styleSheetPaths(stylesDir).map(p => readFileSync(p, 'utf-8'));
    for (const cls of GLYPH_BADGES.filter(c => c !== 'drawer-view-count')) {
      const declared = sheets.some(css =>
        rulesTargeting(css, cls).some(r => r.props.has('--badge-height')));
      if (!declared) missing.push(cls);
    }
    expect(missing).toEqual([]);
  });

  it('keeps a single-digit badge round, with its width on the same box', () => {
    // A 15px design that rounds to 14px of height would otherwise stay 15px
    // wide, an egg rather than a circle. The fallback is the design height,
    // for a browser that never defines the box.
    const round = ['badge', 'section-count-badge', 'drawer-badge', 'ws-picker-badge',
      'thread-status-question-badge'];
    const sheets: string[] = styleSheetPaths(stylesDir).map(p => readFileSync(p, 'utf-8'));
    for (const cls of round) {
      const widths = sheets.flatMap(css => rulesTargeting(css, cls))
        .filter(r => r.selector === `.${cls}`)
        .map(r => r.props.get('min-width') ?? r.props.get('width'))
        .filter((w): w is string => w !== undefined);
      expect(widths, `.${cls} width`).toContain('var(--badge-box, var(--badge-height))');
    }
  });

  it('keeps the drawer attention count round and on the plain counts\' column', () => {
    // The plain count's cell width outranks the base `.badge` width, so the
    // blue circle restates the box. Half the difference goes on its trailing
    // edge, which keeps its middle where a plain number's middle is.
    const drawerCss: string = readFileSync(resolve(stylesDir, 'drawer.css'), 'utf-8');
    const attention = cssRules(drawerCss).find(r => r.selector === '.badge.drawer-view-count');
    expect(attention?.props.get('min-width')).toBe('var(--badge-box, var(--badge-height))');
    expect(attention?.props.get('margin-right'))
      .toBe('calc((var(--badge-height) - var(--badge-box, var(--badge-height))) / 2)');
  });

  it('lets the dots keep the size their own rules give them', () => {
    // Nothing in badges.css may reach them: the shared rule names both only to
    // exclude them, and nothing else names them at all.
    for (const g of GLYPHLESS) {
      const reaching = rulesTargeting(badgesCss, g).filter(r => r.selector !== shared!.selector);
      expect(reaching.map(r => r.selector)).toEqual([]);
    }
  });
});

describe('nothing outranks it', () => {
  it('is the LAST stylesheet main.tsx imports', () => {
    // Its selectors weigh one class, exactly like the rules it re-homes, so
    // source order is the whole of why it wins their `display` and box. An
    // import added below it silently hands the badges back their flex centring.
    const imports: string[] = [...mainTsx.matchAll(/^import '(\.\/styles\/[^']+)';$/gm)]
      .map(m => m[1]);
    expect(imports.length, 'no style imports found').toBeGreaterThan(1);
    expect(imports[imports.length - 1]).toBe('./styles/badges.css');
  });

  it('is the only place a badge is given a display or a box it cannot beat', () => {
    // Every property the shared rule owns. A heavier `padding` shorthand hands
    // back the bottom padding that lifts the baseline, and a heavier
    // `min-height` hands back the unrounded height.
    //
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
          if (!OWNED.some(p => rule.props.has(p))) continue;
          seen++;
          const beats = selectorList(rule.selector)
            .filter(one => one.includes(cls) && (!ordered || one !== `.${cls}`))
            // A dot is outside the shared rule, so its own box is not a rival.
            .filter(one => !GLYPHLESS.some(g => one.includes(`.${g}`)));
          if (beats.length) offenders.push(`${path.split('/src/')[1]}: ${beats.join(', ')}`);
        }
      }
    }
    // A FLOOR, so an empty sweep cannot read as a clean one. Every badge's own
    // rule sets at least one of these, which is what badges.css re-homes.
    expect(seen, 'the sweep found no badge box rules at all').toBeGreaterThanOrEqual(7);
    expect(offenders).toEqual([]);
  });
});

describe('the band the trim measures is the glyph the badge draws', () => {
  it('lets no badge put its font on a pseudo-element', () => {
    // The trim reads the font of the BLOCK CONTAINER, which is the badge. A
    // glyph drawn by a `::after` at its own smaller size is therefore not the
    // band being centred. It sits on that band's baseline, riding as low as the
    // two fonts differ, which the "?" badge did at 2.0px in Chromium and 1.7px
    // in WebKit on a 14px badge. Its font lives on the badge now, and a font
    // moved back onto a pseudo would silently undo it.
    const pseudo = new RegExp(`\\.(${GLYPH_BADGES.join('|')})::?(before|after)\\b`);
    const offenders: string[] = [];
    let seen = 0;
    for (const path of styleSheetPaths(resolve(stylesDir, '..'))) {
      const css: string = readFileSync(path, 'utf-8');
      for (const rule of cssRules(css)) {
        for (const one of selectorList(rule.selector)) {
          // The subject is the last compound, so a rule aimed at a CHILD of the
          // badge is out of scope here: it draws its own box, not the badge's
          // line box.
          if (!pseudo.test(one.split(/[\s>+~]+/).pop() ?? '')) continue;
          seen++;
          const fonts = [...rule.props.keys()].filter(p => /^font(-|$)/.test(p));
          if (fonts.length) offenders.push(`${path.split('/src/')[1]}: ${one} sets ${fonts.join(', ')}`);
        }
      }
    }
    // A FLOOR, so a sweep that matched nothing cannot read as a clean one. The
    // "?" badge's own `content` and colour rules are the ones this counts.
    expect(seen, 'the sweep found no badge pseudo-element rules at all').toBeGreaterThanOrEqual(2);
    expect(offenders).toEqual([]);
  });

  it('keeps the "?" glyph a step smaller than the badge it sits in', () => {
    // The scan above says WHERE the font goes; this says the badge still has
    // one. Dropped, the glyph inherits the body size and outgrows its 0.7rem
    // circle. One rule states it, so there is one band to centre.
    const chatCss: string = readFileSync(resolve(stylesDir, 'chat', 'input-messages.css'), 'utf-8');
    const sized = rulesTargeting(chatCss, 'thread-status-question-badge')
      .filter(r => r.props.has('font-size'));
    expect(sized.map(r => r.props.get('font-size'))).toEqual(['var(--font-size-3xs)']);
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
    // It sits on the wrapper, so it moves both copies of the number.
    const retired = gated.find(r =>
      r.selector === '.list-section-title-collapsible > .section-count');
    expect(retired?.props.get('top')).toBe('0');
    const base = cssRules(drawerCss).find(r =>
      r.selector === '.list-section-title-collapsible > .section-count'
      && r.atRules === '');
    expect(base?.props.get('top'), 'the untrimmed fallback keeps its lift').toBe('-0.0625rem');
  });
});
