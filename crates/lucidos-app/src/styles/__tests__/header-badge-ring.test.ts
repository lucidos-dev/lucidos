import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { REDUCED_MOTION_ROOT, block, cssRules, decl, isReducedMotionRule, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const baseCss = styles('global/base.css');
const markCss = styles('header-mark.css');
const mobileCss = styles('mobile.css');

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
    expect(decl(headerBadge, 'background')).toBe('var(--header-badge-bg)');
    expect(decl(headerBadge, 'box-shadow'))
      .toBe('0 0 0 var(--header-badge-ring-width) '
        + 'color-mix(in srgb, var(--header-fg) var(--header-control-veil), var(--header-badge-ring))');
  });

  it('is a box-shadow, never a border', () => {
    expect(decl(headerBadge, 'border'), 'a border would resize the badge').toBeNull();
    expect(decl(headerBadge, 'outline')).toBeNull();
  });

  it('cancels its own transition under reduced motion, in this sheet', () => {
    expect(decl(headerBadge, 'transition')).toBe('box-shadow var(--duration-normal)');
    const cancel = cssRules(shellCss).find(r =>
      r.selector === `${REDUCED_MOTION_ROOT} .app-header .badge` && isReducedMotionRule(r));
    expect(cancel?.props.get('transition'), 'no reduced-motion cancel in shell.css').toBe('none');
  });

  it('is NOT duplicated onto the mark, which inherits it', () => {
    expect(markCss).not.toContain('--header-mark-badge-ring');
    expect(block(markCss, '\n.brand-mark-slot {')).not.toContain('box-shadow');
  });
});

describe('the veil a control wears is the veil its badge ring paints', () => {
  const WASH = 'color-mix(in srgb, var(--header-fg) var(--header-control-veil), transparent)';
  const STEPS = [
    'var(--header-control-veil-hover)',
    'var(--header-control-veil-active)',
    'var(--header-control-veil-active-hover)',
  ];

  const onTheBar = /\.(app|pane)-header(?![\w-])/;
  const washing = allSheets().flatMap(([file, css]) =>
    rulesTargeting(css, 'icon-btn')
      .filter(r => onTheBar.test(r.selector))
      .filter(r => r.props.has('background') || r.props.has('background-color'))
      .map(r => ({ file, r })));

  it('rests at nothing on the bar itself', () => {
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
    expect(washing.length, 'hovered, toggled on, and both at once')
      .toBeGreaterThanOrEqual(3);
    for (const { file, r } of washing) {
      expect(r.props.get('background'), `${file}: ${r.selector} paints its own wash`).toBe(WASH);
      expect(r.props.get('background-color'), `${file}: ${r.selector}`).toBeUndefined();
    }
  });

  it('is declared by the rule that washes it, on the same element', () => {
    for (const { file, r } of washing) {
      expect(STEPS, `${file}: ${r.selector} washes an alpha it did not declare`)
        .toContain(r.props.get('--header-control-veil'));
    }
  });

  it('never names a raw alpha away from the four steps', () => {
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
    const slot = cssRules(markCss)
      .find(r => r.selector === '.app-header .brand-mark-slot:has(.icon-btn:hover)');
    expect(slot?.props.get('--header-control-veil'))
      .toBe('var(--header-control-veil-hover)');
    expect(slot?.atRules, 'hover latches after a tap on touch').toContain('(hover: hover)');
  });
});

describe('the ring colour is derived from the bar, never written out', () => {
  it('mixes the gradient\'s own two stops', () => {
    const ring = decl(block(shellCss, '.pane-header {'), '--header-badge-ring');
    expect(ring).toContain('color-mix');
    expect(ring).toContain('var(--header-bar-top)');
    expect(ring).toContain('var(--header-bar-bottom)');
    expect(ring, 'a transparent ring would show the glyph through it')
      .not.toContain('transparent');
  });

  it('every theme that sets the bar names its two stops', () => {
    const gradients = baseCss.match(/--header-gradient:[^;]+;/g) ?? [];
    expect(gradients.length, 'expected the dark, iOS-PWA and light bars').toBe(3);
    for (const g of gradients) {
      expect(g, `${g} must build from the named stops`)
        .toContain('var(--header-bar-top)');
      expect(g).toContain('var(--header-bar-bottom)');
    }
    expect((baseCss.match(/--header-bar-top:/g) ?? []).length).toBe(3);
    expect((baseCss.match(/--header-bar-bottom:/g) ?? []).length).toBe(3);
  });

  it('the macOS title-bar strip takes the top stop, in every theme', () => {
    const strips = baseCss.match(/--titlebar-strip-bg:[^;]+;/g) ?? [];
    expect(strips.length, 'expected the dark, iOS-PWA and light strips').toBe(3);
    for (const s of strips) {
      expect(s, `${s} must take the named top stop`).toContain('var(--header-bar-top)');
    }
  });
});
