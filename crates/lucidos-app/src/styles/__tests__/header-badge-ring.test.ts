/**
 * Source scans over the ring that keeps a header badge from FUSING with the
 * glyph it rides.
 *
 * The reported bug: the Lucidos mark's unread count and the mark's own
 * bottom-right square are both painted `--header-badge-bg` (white). They
 * merged into one shape, and the pair read as a teardrop. The bell had the
 * same collision with its arc, more mildly.
 *
 * These are properties of the STYLESHEET, which is the only place the fix
 * exists. Nothing renders in a unit test, and no e2e can assert "these two
 * white shapes look like one".
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const baseCss = styles('global/base.css');
const markCss = styles('header-mark.css');
const mobileCss = styles('mobile.css');

/** Every stylesheet in the app, as `[relative path, source]`.
 *
 *  The veil scans below sweep the whole of `src/`, not a named list and not
 *  `src/styles` alone. The escape they guard against is a header wash written
 *  in ANOTHER sheet: `.claude/rules/frontend-css.md` puts the `.icon-btn`
 *  state variants in `global/host-components.css`, and a component may keep
 *  its own sheet beside itself (`components/search/SearchEverywhere.css`). A
 *  list naming only `panels/shell.css` calls the tree clean while the wash
 *  drifts next door. */
function allSheets(): Array<[string, string]> {
  const out: Array<[string, string]> = [];
  const walk = (dir: string, rel: string): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full: string = resolve(dir, entry.name);
      const path: string = rel ? `${rel}/${entry.name}` : entry.name;
      if (entry.isDirectory()) {
        if (entry.name !== '__tests__') walk(full, path);
      } else if (entry.name.endsWith('.css')) {
        out.push([path, readFileSync(full, 'utf-8')]);
      }
    }
  };
  walk(resolve(stylesDir, '..'), '');
  return out;
}

describe('the ring rides every badge on the bar, from one rule', () => {
  const headerBadge = block(shellCss, '.app-header .badge {');

  it('is applied where the white paint is, so the two cannot drift apart', () => {
    // White-on-blue is what makes a badge legible against the BAR; the ring is
    // what stops that same white fusing with the GLYPH. One rule owns both, so
    // a badge can never get the paint without the separation.
    expect(decl(headerBadge, 'background')).toBe('var(--header-badge-bg)');
    expect(decl(headerBadge, 'box-shadow'))
      .toBe('0 0 0 var(--header-badge-ring-width) '
        + 'color-mix(in srgb, var(--header-fg) var(--header-control-veil), var(--header-badge-ring))');
  });

  it('is a box-shadow, never a border', () => {
    // A border grows the badge's box. Both of the mark's badges are placed by
    // a `calc()` against that box. A border would shift them off the glyph
    // corner they are derived to sit on.
    expect(decl(headerBadge, 'border'), 'a border would resize the badge').toBeNull();
    expect(decl(headerBadge, 'outline')).toBeNull();
  });

  it('cancels its own transition under reduced motion, in this sheet', () => {
    // global/modal-overlay.css cancels the BUTTON's transitions centrally. The
    // ring's cancel cannot join that list. main.tsx imports that sheet before
    // this one, and both selectors are (0,2,0). The copy there would lose the
    // tie, and the ring would fade against a button that snapped.
    expect(decl(headerBadge, 'transition')).toBe('box-shadow var(--duration-normal)');
    const cancel = cssRules(shellCss).find(r =>
      r.selector === '.app-header .badge' && r.atRules.includes('prefers-reduced-motion'));
    expect(cancel?.props.get('transition'), 'no reduced-motion cancel in shell.css').toBe('none');
  });

  it('is NOT duplicated onto the mark, which inherits it', () => {
    // The fusing is a property of the bar, not of the mark: the bell has it
    // too. A second copy scoped to the mark is how the bell would silently
    // stop matching it.
    expect(markCss).not.toContain('--header-mark-badge-ring');
    expect(block(markCss, '\n.brand-mark-slot {')).not.toContain('box-shadow');
  });
});

describe('the veil a control wears is the veil its badge ring paints', () => {
  // Reported: the thread filter's count badge notched a dark hole in the
  // lighter square the toggled button had just drawn. The sheet painted the
  // ring the bare bar. It painted the button the bar plus a white wash. The
  // two matched only at rest.
  const WASH = 'color-mix(in srgb, var(--header-fg) var(--header-control-veil), transparent)';
  const STEPS = [
    'var(--header-control-veil-hover)',
    'var(--header-control-veil-active)',
    'var(--header-control-veil-active-hover)',
  ];

  /** Names the bar element itself. Both class names, because the bar wears
   *  both: it is `<header class="pane-header app-header">` (AppHeader.tsx).
   *  Matching only `.app-header` would let `.pane-header .icon-btn:hover` wash
   *  the same buttons at the same specificity, unseen by every scan here.
   *
   *  Naming the bar is the limit of what these scans see. A wash hung off a
   *  container INSIDE it, say `.mobile-header-row .icon-btn:hover`, escapes
   *  them. Telling those containers from the rest of the app needs a
   *  hand-kept list, and a list that drifts reads as coverage it lost. */
  const onTheBar = /\.(app|pane)-header(?![\w-])/;
  const washing = allSheets().flatMap(([file, css]) =>
    rulesTargeting(css, 'icon-btn')
      .filter(r => onTheBar.test(r.selector))
      .filter(r => r.props.has('background') || r.props.has('background-color'))
      .map(r => ({ file, r })));

  it('rests at nothing on the bar itself', () => {
    // Rules whose SUBJECT is the bar, over every sheet. Raising the veil there
    // instead of on a control washes every ring on the bar for good. The scans
    // below stay green through it, because no control rule changed.
    //
    // `rulesTargeting` rather than "no whitespace in the member", which reads
    // an ancestor-qualified `:root[data-keyboard-active] .app-header` as a
    // descendant and skips it. mobile.css already writes that shape.
    const bar = allSheets().flatMap(([file, css]) => {
      const seen = new Set<string>();
      return [...rulesTargeting(css, 'app-header'), ...rulesTargeting(css, 'pane-header')]
        .filter(r => r.props.has('--header-control-veil'))
        .filter(r => !seen.has(`${r.atRules}|${r.selector}`)
          && (seen.add(`${r.atRules}|${r.selector}`), true))
        .map(r => ({ file, r }));
    });
    expect(bar.length, 'one resting default, on the bar').toBe(1);
    expect(bar[0].r.selector).toBe('.pane-header');
    expect(bar[0].r.props.get('--header-control-veil')).toBe('0%');
  });

  it('is the only thing a header control paints its background with', () => {
    // The guard has to be the POSITIVE one. "No rgba() in this sheet" passes
    // for `rgb(255 255 255 / 0.3)`, for `#ffffff4d`, and for the same wash
    // written next door in global/host-components.css.
    // A FLOOR, not a count: an empty sweep would make the loops below vacuous.
    // A ceiling would fail on a correct edit, such as splitting the grouped
    // rule or washing a fourth state.
    expect(washing.length, 'hovered, toggled on, and both at once')
      .toBeGreaterThanOrEqual(3);
    for (const { file, r } of washing) {
      expect(r.props.get('background'), `${file}: ${r.selector} paints its own wash`).toBe(WASH);
      expect(r.props.get('background-color'), `${file}: ${r.selector}`).toBeUndefined();
    }
  });

  it('is declared by the rule that washes it, on the same element', () => {
    // A wash reading an alpha its own rule never set reads the bar's resting
    // 0%, so the button stays bare while its ring tracks nothing.
    for (const { file, r } of washing) {
      expect(STEPS, `${file}: ${r.selector} washes an alpha it did not declare`)
        .toContain(r.props.get('--header-control-veil'));
    }
  });

  it('never names a raw alpha away from the four steps', () => {
    // One number per state. A literal written at a use site is how the ring
    // and the square drifted apart the first time.
    const allowed = [...STEPS, '0%', 'inherit'];
    for (const [file, css] of allSheets()) {
      for (const r of cssRules(css)) {
        const set = r.props.get('--header-control-veil');
        if (set === undefined) continue;
        expect(allowed, `${file}: ${r.selector} sets ${set}`).toContain(set);
      }
    }
  });

  it('drops where a badge overhangs its control, and returns where it does not', () => {
    // mobile.css re-corners every badge ONTO the button's edge, so roughly
    // half its ring falls on the bare bar and no veil matches both halves. The
    // mark's two badges override that corner to stay inside their host. They
    // override the veil with it.
    const corner = cssRules(mobileCss).find(r => r.selector === '.badge' && r.props.has('top'));
    expect(corner?.props.get('--header-control-veil'),
      'the mobile corner reset must drop the veil with it').toBe('0%');
    for (const cls of ['brand-badge', 'brand-unread-badge']) {
      const rule = cssRules(markCss).find(r => r.selector === `.brand-mark-slot .badge.${cls}`);
      expect(rule?.props.get('--header-control-veil'), `${cls} stays inside its host`)
        .toBe('inherit');
    }
  });

  it("reaches the mark's badges, which are its SIBLINGS and not its children", () => {
    // A custom property only travels DOWN. HeaderMark puts the badges beside
    // the button, not inside it. So the slot the three share is the only
    // element that can raise the veil for all of them.
    const slot = cssRules(markCss)
      .find(r => r.selector === '.app-header .brand-mark-slot:has(.icon-btn:hover)');
    expect(slot?.props.get('--header-control-veil'))
      .toBe('var(--header-control-veil-hover)');
    expect(slot?.atRules, 'hover latches after a tap on touch').toContain('(hover: hover)');
  });
});

describe('the ring colour is derived from the bar, never written out', () => {
  it('mixes the gradient\'s own two stops', () => {
    // A literal would be right in exactly one theme and would drift the first
    // time a stop moved. Painted, not transparent: what sits behind a badge is
    // the glyph, so a see-through ring separates nothing.
    const ring = decl(block(shellCss, '.pane-header {'), '--header-badge-ring');
    expect(ring).toContain('color-mix');
    expect(ring).toContain('var(--header-bar-top)');
    expect(ring).toContain('var(--header-bar-bottom)');
    expect(ring, 'a transparent ring would show the glyph through it')
      .not.toContain('transparent');
  });

  it('every theme that sets the bar names its two stops', () => {
    // The mix above resolves per theme. Set the gradient from literals in one
    // theme and the ring there points at an undefined var. The badge then
    // loses its separation in that theme alone.
    const gradients = baseCss.match(/--header-gradient:[^;]+;/g) ?? [];
    expect(gradients.length, 'expected the dark, iOS-PWA and light bars').toBe(3);
    for (const g of gradients) {
      expect(g, `${g} must build from the named stops`)
        .toContain('var(--header-bar-top)');
      expect(g).toContain('var(--header-bar-bottom)');
    }
    // Named as many times as they are used, so no theme inherits another's bar.
    expect((baseCss.match(/--header-bar-top:/g) ?? []).length).toBe(3);
    expect((baseCss.match(/--header-bar-bottom:/g) ?? []).length).toBe(3);
  });

  it('the macOS title-bar strip takes the top stop, in every theme', () => {
    // The strip butts against the header's top edge and has to be seamless
    // with it. It is the other consumer of the named stops, and a literal here
    // is the drift the naming exists to prevent.
    const strips = baseCss.match(/--titlebar-strip-bg:[^;]+;/g) ?? [];
    expect(strips.length, 'expected the dark, iOS-PWA and light strips').toBe(3);
    for (const s of strips) {
      expect(s, `${s} must take the named top stop`).toContain('var(--header-bar-top)');
    }
  });
});
