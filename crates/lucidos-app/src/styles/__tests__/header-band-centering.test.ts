import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { UI_SCALE_MIN } from '@lucidos/appearance';
import { block, cssRules, decl, focusRingWidthRem, rulesTargeting, selectorList, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const drawerCss = styles('drawer.css');
const shellRules = cssRules(shellCss);

const DESKTOP = '@media (--desktop-layout)';

function desktopRule(selector: string): CssRule {
  const found = shellRules.filter(r => r.selector === selector && r.atRules === DESKTOP);
  expect(found.length, `expected exactly one desktop \`${selector}\` rule`).toBe(1);
  return found[0];
}

const CENTER_ON_BAND = 'translateY(calc(-50% - var(--header-band-lift)))';

describe('the desktop header centers on the whole bar, not on the header alone', () => {
  it('the lift is half the reclaimed title-bar band, so it is 0px off the macOS Tauri build', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--header-band-lift')).toBe('calc(var(--titlebar-inset, 0px) / 2)');
  });

  it.each([
    ['.threads-header'],
    ['.app-header .pane-header-brand'],
    ['.app-header .content-header-elements'],
    ['.thread-toggle-slot'],
  ])('%s takes the lift', selector => {
    expect(desktopRule(selector).props.get('transform')).toBe(CENTER_ON_BAND);
  });

  it('the Filter button rides its row, with no vertical term of its own', () => {
    const slotRules = shellRules.filter(
      r => r.selector.includes('.filter-slot') && r.atRules === DESKTOP,
    );
    for (const rule of slotRules) {
      for (const prop of ['top', 'bottom', 'transform', 'margin-top', 'position']) {
        expect(rule.props.get(prop), `${rule.selector} { ${prop} }`).toBeUndefined();
      }
    }

    const base = shellRules.find(r => r.selector === '.filter-slot' && r.atRules === '');
    for (const prop of ['top', 'bottom', 'transform', 'margin-top', 'position']) {
      expect(base?.props.get(prop), `.filter-slot { ${prop} }`).toBeUndefined();
    }
  });
});

describe('the brand region is as tall as the label it centres', () => {
  it('the region reserves the label height, so a retuned mark cannot overflow it', () => {
    const brand = desktopRule('.app-header .pane-header-brand');
    expect(brand.props.get('min-height'))
      .toBe('max(var(--header-icon-box), var(--header-mark-tap))');
    expect(brand.props.get('height'), 'a fixed height would re-open the overflow')
      .toBeUndefined();
  });

  it('both terms are the real box heights, not numbers that once matched them', () => {
    const markRules = cssRules(readFileSync(resolve(stylesDir, 'header-mark.css'), 'utf-8'));
    const mark = markRules.find(r => r.selector === '.brand-mark');
    expect(mark?.props.get('height'), '.brand-mark drifted from --header-mark-tap')
      .toBe('var(--header-mark-tap)');
  });
});

describe('on the overlay build the leading control only steps sideways', () => {
  it('the lights reserve is derived from the x the shell places them at', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--titlebar-lights-reserve')).toBe(
      'calc(var(--titlebar-lights-x, 10px) + var(--titlebar-lights-cluster)'
      + ' + var(--titlebar-lights-gap))',
    );
  });

  it('every term of the reserve is px, because the lights do not scale with the UI', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    expect(/var\(--titlebar-lights-x, (\d+px)\)/.exec(reserve)?.[1]).toMatch(/^\d+px$/);
    for (const term of ['--titlebar-lights-cluster', '--titlebar-lights-gap']) {
      expect(root?.props.get(term), term).toMatch(/^\d+px$/);
    }
  });

  it('the derived reserve still comes out at the 80px every reader was sized for', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const terms = [...reserve.matchAll(/(\d+)px/g)].map(m => Number(m[1]));
    const [x] = terms;
    const cluster = parseInt(root!.props.get('--titlebar-lights-cluster')!, 10);
    const gap = parseInt(root!.props.get('--titlebar-lights-gap')!, 10);
    expect(x + cluster + gap).toBe(80);
  });

  it('splits that slack evenly, so the lights are centred in the room they get', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const x = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(parseInt(root!.props.get('--titlebar-lights-gap')!, 10)).toBe(x);
  });

  it('the reserve is registered, so its JS reader still gets a length', () => {
    const body = block(shellCss, '@property --titlebar-lights-reserve');
    expect(decl(body, 'syntax')).toBe("'<length>'");
    expect(decl(body, 'initial-value')).toBe('80px');
    const at = shellCss.indexOf('@property --titlebar-lights-reserve');
    expect(at, 'the registration is missing').toBeGreaterThanOrEqual(0);
    expect(at, 'the registration must precede the desktop media query')
      .toBeLessThan(shellCss.indexOf(`${DESKTOP} {`));
  });

  it('the fallback x is the x the shell actually places the lights at', () => {
    const rust = readFileSync(resolve(stylesDir, '../traffic_lights.rs'), 'utf-8');
    const placed = Number(/LIGHTS_X_PX:\s*f64\s*=\s*([\d.]+)/.exec(rust)?.[1]);
    expect(placed, 'LIGHTS_X_PX not found in traffic_lights.rs').toBeGreaterThan(0);

    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const reserve = root!.props.get('--titlebar-lights-reserve')!;
    const fallback = Number(/var\(--titlebar-lights-x, (\d+)px\)/.exec(reserve)?.[1]);
    expect(fallback).toBe(placed);
  });

  it('the reserve still clears a control that is centred on the bar', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const button = cssRules(styles('global/host-components.css'))
      .filter(r => selectorList(r.selector).includes('.icon-btn.header-icon'));
    const iconBox = button.find(r => r.props.has('height'));
    expect(iconBox, 'no .icon-btn.header-icon rule declaring a height').toBeDefined();
    const height = iconBox!.props.get('height')!;
    const named = /^var\((--[\w-]+)\)$/.exec(height)?.[1];
    const baseRoot = cssRules(styles('global/base.css')).filter(r => r.selector === ':root' && !r.atRules);
    const resolved = named
      ? [...button, ...baseRoot].map(r => r.props.get(named)).find(v => v !== undefined)
      : height;
    expect(resolved, `${named} is declared on no .icon-btn.header-icon rule and not on :root`).toBeDefined();
    expect(root?.props.get('--header-icon-box'), '--header-icon-box drifted from the button')
      .toBe(resolved);
  });

  it('the drawer toggle clears the lights, and moves in no other direction', () => {
    expect(desktopRule(':root[data-titlebar-overlay]').props.get('--header-lead-inset'))
      .toBe('var(--titlebar-lights-reserve)');
    expect(shellRules.filter(
      r => r.selector === ':root[data-titlebar-overlay] .thread-toggle-slot',
    ), 'the resting place has two declarations again').toEqual([]);
    expect(desktopRule('.thread-toggle-slot').props.get('left'))
      .toBe('var(--header-lead-inset)');
    expect(shellCss, '--thread-toggle-home is retired; one name for one place')
      .not.toContain('--thread-toggle-home');
  });

  it('the reserve the centred cluster clears is the WIDER of the row\'s two ends', () => {
    const reserve = desktopRule(':root').props.get('--brand-side-reserve')!;
    expect(reserve.startsWith('max('), `--brand-side-reserve is not a max(): ${reserve}`)
      .toBe(true);
    expect(reserve).toContain('calc(var(--header-lead-inset) + 2 * var(--header-icon-box) + 0.25rem)');
    expect(reserve).toContain('var(--brand-trail-end)');
    expect(desktopRule(':root').props.get('--brand-trail-end')).toContain('2 * var(--header-icon-box)');
  });

  it('the cluster\'s floor IS its natural width, so the chevrons hug the mark', () => {
    expect(desktopRule(':root').props.get('--desktop-nav-min-span'))
      .toBe('calc(2 * var(--header-icon-box) + var(--header-mark-tap))');
  });

  it('the content row keeps the reserve as a FLOOR under the divider', () => {
    const base = desktopRule('.app-header .content-header-elements').props.get('left');
    expect(base).toBe('calc(var(--divider-x) + var(--divider-width))');

    const rule = desktopRule(':root[data-titlebar-overlay] .app-header .content-header-elements');
    expect(rule.props.get('left')).toBeUndefined();
    expect(rule.props.get('padding-left')).toBe('var(--content-leading-inset)');
    expect(rule.props.get('--content-leading-inset')).toBe(
      'max(0px, calc(var(--titlebar-lights-reserve) - var(--divider-x) - var(--divider-width)))',
    );
    expect([...rule.props.keys()], 'horizontal-only here too').toEqual(['--content-leading-inset', 'padding-left']);
  });

  it('the drawer row leads with the toggle\'s room, which carries the lights reserve', () => {
    expect(desktopRule('.threads-header').props.get('--threads-row-lead'))
      .toContain('var(--header-lead-inset)');
    expect(shellRules.filter(
      r => r.selector === ':root[data-titlebar-overlay] .threads-header',
    ), 'the row has an overlay-only lead again').toEqual([]);
  });

  it('the lead survives the search field taking over the row', () => {
    const searchRules = [...shellRules, ...cssRules(drawerCss)].filter(
      r => r.selector.includes('.threads-header') && r.selector.includes('.search-active'),
    );
    expect(searchRules.length, 'the row has no search-active rule to guard').toBeGreaterThan(0);
    for (const rule of searchRules) {
      for (const prop of ['padding', 'padding-left', 'clip-path']) {
        expect(rule.props.get(prop), `${rule.selector} { ${prop} }`).toBeUndefined();
      }
    }
  });

  it('every rule that moves a control is gated on the attribute, never on the var', () => {
    const movers = shellRules.filter(
      r => r.atRules === DESKTOP && r.props.get('--titlebar-lights-reserve') === undefined
        && [...r.props.values()].some(v => v.includes('--titlebar-lights-reserve')),
    );
    expect(
      movers.length,
      'expected the toggle\'s inset and the Canvas row\'s floor',
    ).toBe(2);
    for (const rule of movers) {
      expect(rule.selector, rule.selector).toContain('[data-titlebar-overlay]');
    }
  });
});

describe('the Threads title is centred on the pane, not between the icons', () => {
  it('is out of the flex row and pinned to the pane\'s own middle', () => {
    const title = desktopRule('.threads-header-title');
    expect(title.props.get('position')).toBe('absolute');
    expect(title.props.get('left')).toBe('50%');
    expect(title.props.get('transform')).toBe('translate(-50%, -50%)');
    expect(title.props.get('flex'), 'flex:1 is what centred it on the gap').toBeUndefined();
    expect(title.props.get('top')).toBe('50%');
  });

  it('packs Grouping and Search at the row\'s trailing edge, and Filter at its leading one', () => {
    expect(desktopRule('.threads-header').props.get('justify-content')).toBe('flex-end');
    expect(desktopRule('.threads-header .filter-slot').props.get('margin-right')).toBe('auto');
  });

  it('clears the WIDER of the row\'s two ends on BOTH sides, on both builds', () => {
    expect(desktopRule('.threads-header-title').props.get('max-width')).toBe(
      'calc(100% - 2 * max(var(--threads-row-lead) + var(--header-icon-box) + var(--pane-header-gap), '
        + '0.5rem + 2 * (var(--header-icon-box) + var(--pane-header-gap))))',
    );
    expect(shellRules.filter(
      r => r.selector === ':root[data-titlebar-overlay] .threads-header-title',
    ), 'the title has an overlay-only clamp again').toEqual([]);
  });

  it('counts the same two ends the row actually keeps clear', () => {
    const clamp = desktopRule('.threads-header-title').props.get('max-width')!;
    const [, lead, trailPad] = /max\((var\([\w-]+\)) [^,]+, ([\d.]+rem) \+/.exec(clamp) ?? [];
    expect(desktopRule('.threads-header').props.get('padding'))
      .toBe(`0 ${trailPad} 0 ${lead}`);
  });

  it('declares the lead on the row, where --pane-header-gap actually resolves', () => {
    const declaring = shellRules.filter(r => r.props.has('--threads-row-lead'));
    expect(declaring.map(r => r.selector)).toEqual(['.threads-header']);
    expect(shellCss, '--threads-title-lead is retired; the row owns the lead')
      .not.toContain('--threads-title-lead');
  });
});

describe('the threads header clips sideways only, on both builds', () => {
  it('its top and bottom let out the focus ring, and its right edge is flush', () => {
    const clip = desktopRule('.threads-header').props.get('clip-path') ?? '';
    const [, top, right, bottom] = /^inset\((-?[\d.]+(?:rem)?) (-?[\d.]+(?:rem)?) (-?[\d.]+(?:rem)?) /
      .exec(clip) ?? [];
    const band = focusRingWidthRem(styles('global/base.css'));
    for (const edge of [top, bottom]) {
      expect(-parseFloat(edge), `clip ${clip} cuts the ${band}rem focus ring`).toBeGreaterThanOrEqual(band);
    }
    expect(right).toBe('0');
  });

  it('carries no clip release for the popout that no longer exists', () => {
    expect(shellCss).not.toContain('thread-filter-dropdown');
  });

  it('nothing puts an overflow clip back on it', () => {
    for (const css of [shellCss, drawerCss]) {
      for (const rule of rulesTargeting(css, 'threads-header')) {
        for (const prop of ['overflow', 'overflow-y']) {
          const value = rule.props.get(prop);
          expect(value ?? 'visible', `${rule.selector} { ${prop} }`).toBe('visible');
        }
      }
    }
  });
});

describe('both desktop builds show one bar of the same height', () => {
  it('web and PWA get the whole bar from the header, having no band', () => {
    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    expect(root?.props.get('--desktop-bar-height')).toBe('3rem');
    expect(root?.props.get('--app-header-height')).toBe('var(--desktop-bar-height)');
  });

  it('the bar still clears the fixed band at the smallest UI scale', () => {
    const minScale = UI_SCALE_MIN;

    const root = shellRules.find(r => r.selector === ':root' && r.atRules === DESKTOP);
    const barRem = Number(/^([\d.]+)rem$/.exec(root!.props.get('--desktop-bar-height')!)?.[1]);
    expect(barRem, '--desktop-bar-height is no longer a plain rem').toBeGreaterThan(0);

    const TITLEBAR_INSET_PX = 28;
    const barAtMinScale = barRem * 16 * (minScale / 100);
    expect(
      barAtMinScale,
      `a ${barRem}rem bar is ${barAtMinScale}px at ${minScale}%, under the ${TITLEBAR_INSET_PX}px band`,
    ).toBeGreaterThan(TITLEBAR_INSET_PX);
  });

  it('the overlay build SUBTRACTS its band, so the sum holds at every UI scale', () => {
    expect(desktopRule(':root[data-titlebar-overlay]').props.get('--app-header-height'))
      .toBe('calc(var(--desktop-bar-height) - var(--titlebar-inset, 0px))');
  });

  it('no mobile viewport is touched', () => {
    for (const rule of shellRules) {
      if (rule.props.get('--app-header-height') === undefined) continue;
      expect(rule.atRules, `${rule.selector} { --app-header-height }`).toBe(DESKTOP);
    }
  });
});

const ROUNDED = `${DESKTOP} @supports (width: round(1.5px, 1px))`;

describe('the regions holding a badge centre on whole pixels', () => {
  function roundedRule(selector: string): { rule: CssRule; index: number } {
    const index = shellRules.findIndex(
      r => r.atRules === ROUNDED && selectorList(r.selector).includes(selector),
    );
    expect(index, `expected \`${selector}\` in the rounded block`).toBeGreaterThanOrEqual(0);
    return { rule: shellRules[index], index };
  }

  it.each([
    ['.threads-header', 'translateY(round(calc(-50% - var(--header-band-lift)), 1px))'],
    ['.app-header .pane-header-brand', 'translateY(round(calc(-50% - var(--header-band-lift)), 1px))'],
    ['.app-header .content-header-elements', 'translateY(round(calc(-50% - var(--header-band-lift)), 1px))'],
    ['.thread-toggle-slot', 'translateY(round(calc(-50% - var(--header-band-lift)), 1px))'],
    ['.app-header .pane-header-brand .pane-header-brand-label', 'translate(round(-50%, 1px), round(-50%, 1px))'],
  ])('%s rounds, after the rule it overrides', (selector, rounded) => {
    const { rule, index } = roundedRule(selector);
    expect(rule.props.get('transform')).toBe(rounded);
    expect(index, 'an earlier rule would lose on source order').toBeGreaterThan(
      shellRules.indexOf(desktopRule(selector)),
    );
  });

  it('never rounds outside the gate', () => {
    const ungated = shellRules.filter(
      r => !r.atRules.includes('@supports') && (r.props.get('transform') ?? '').includes('round('),
    );
    expect(ungated.map(r => r.selector)).toEqual([]);
  });
});
