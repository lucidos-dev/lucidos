import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { UI_SCALE_MAX, UI_SCALE_MIN, UI_SCALE_STEP } from '@lucidos/appearance';
import { clientSourcePaths, cssRules, decl, rulesTargeting, selectorList, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const badgesCss: string = readFileSync(resolve(stylesDir, 'badges.css'), 'utf-8');
const headerMarkCss: string = readFileSync(resolve(stylesDir, 'header-mark.css'), 'utf-8');
const iconsTsx: string = readFileSync(
  resolve(stylesDir, '..', 'components', 'shared', 'icons.tsx'), 'utf-8');
const mainTsx: string = readFileSync(resolve(stylesDir, '..', 'main.tsx'), 'utf-8');

const iconRules = cssRules(badgesCss)
  .filter(r => r.selector === '.badge.badge-icon');

const MIN_CLEARANCE_SHARE = 0.075;

function inkReach(component: string): number {
  const svg = iconsTsx.match(new RegExp(`export function ${component}\\(\\)[\\s\\S]*?</svg>`))?.[0] ?? '';
  const view = Number(svg.match(/viewBox="0 0 (\d+) \d+"/)?.[1]);
  const stroke = Number(svg.match(/stroke-width="([\d.]+)"/)?.[1]);
  const points: Array<[number, number]> = [];
  for (const [, list] of svg.matchAll(/points="([^"]+)"/g)) {
    const n = list.trim().split(/[\s,]+/).map(Number);
    for (let i = 0; i < n.length; i += 2) points.push([n[i], n[i + 1]]);
  }
  for (const [, d] of svg.matchAll(/ d="([^"]+)"/g)) {
    let x = 0, y = 0;
    for (const [, cmd, args] of d.matchAll(/([MLHVAmlhva])([^MLHVAmlhva]*)/g)) {
      const n = args.trim().split(/[\s,]+|(?=-)/).filter(Boolean).map(Number);
      const step = { M: 2, L: 2, H: 1, V: 1, A: 7 }[cmd.toUpperCase() as 'M'];
      for (let i = 0; i < n.length; i += step) {
        const rel = cmd === cmd.toLowerCase();
        const c = cmd.toUpperCase();
        if (c === 'H') x = (rel ? x : 0) + n[i];
        else if (c === 'V') y = (rel ? y : 0) + n[i];
        else { x = (rel ? x : 0) + n[i + step - 2]; y = (rel ? y : 0) + n[i + step - 1]; }
        points.push([x, y]);
      }
    }
  }
  expect(points.length, `${component} has no vertices to read`).toBeGreaterThan(0);
  const half = view / 2;
  return Math.max(...points.map(([px, py]) => Math.hypot(px - half, py - half) + stroke / 2)) / view;
}

const BADGE_ICON_REACH: Record<string, number> = {
  HourglassIcon: inkReach('HourglassIcon'),
  ReloadIcon: inkReach('ReloadIcon'),
};

const GLYPH_BADGES = [
  'badge',
  'section-count-badge',
  'drawer-badge',
  'brand-menu-ws-badge',
  'ws-picker-badge',
  'thread-status-question-badge',
  'app-store-status',
  'app-store-modified-chip',
  'theme-card-badge',
];

const GLYPHLESS = ['brand-badge-dot', 'system-attention-badge-corner'];

const SHARED_SELECTORS = GLYPH_BADGES.map(c => c === 'badge'
  ? `.badge:not(:where(${[...GLYPHLESS, 'badge-icon'].map(g => `.${g}`).join(', ')}))`
  : `.${c}`);

const OWNED = ['display', 'align-content', 'height', 'min-height', 'padding',
  'padding-top', 'padding-bottom', 'padding-block'];

const shared = cssRules(badgesCss).find(r => r.props.get('text-box-trim') === 'trim-both');
const tracking = cssRules(badgesCss).find(r => r.props.get('letter-spacing') === 'normal');

describe('one rule centres the glyph in every badge that has one', () => {
  it('trims the line box to the cap band, and parks it on a whole-pixel baseline', () => {
    expect(shared, 'no trim rule in badges.css').toBeDefined();
    expect(shared!.props.get('text-box-edge')).toBe('cap alphabetic');
    expect(shared!.props.get('align-content')).toBe('end');
    expect(shared!.props.get('text-align')).toBe('center');
  });

  it('stops each badge being a flex box, which is what lets the trim land', () => {
    expect(shared!.props.get('display')).toBe('inline-block');
  });

  it('is gated on support, so an older browser keeps the flex centring', () => {
    expect(shared!.atRules).toBe('@supports (text-box-trim: trim-both)');
  });

  it('names every badge that carries a number or a sign, and no dot', () => {
    expect(selectorList(shared!.selector)).toEqual(SHARED_SELECTORS);
  });

  it('takes no tracking from its host, and takes it UNGATED', () => {
    expect(tracking, 'no letter-spacing rule in badges.css').toBeDefined();
    expect(tracking!.atRules, 'the lean is there with or without the trim').toBe('');
    expect(selectorList(tracking!.selector)).toEqual(GLYPH_BADGES.map(c => `.${c}`));
  });

  it('gives icons a square independent of text metrics, even without trim support', () => {
    const icon = iconRules.find(r => r.atRules === '');
    expect(icon?.props.get('display')).toBe('grid');
    expect(icon?.props.get('place-items')).toBe('center');
    expect(icon?.props.get('--badge-icon-box')).toBe('var(--badge-height)');
    expect(icon?.props.get('padding')).toBe('var(--badge-icon-inset)');
    for (const side of ['width', 'height']) {
      expect(icon?.props.get(side)).toBe('var(--badge-icon-box)');
    }
    expect(iconRules.some(r => /cap|baseline|--badge-box/.test([...r.props.values()].join(' ')))).toBe(false);
  });

  it('snaps the box and symmetric insets independently of text trim', () => {
    const icon = iconRules.find(r => r.atRules === '@supports (width: round(down, 1rem, 1px))');
    expect(icon?.props.get('--badge-icon-box')).toBe('round(down, var(--badge-height), 1px)');
    expect(icon?.props.get('--badge-icon-inset'))
      .toBe('round(down, var(--badge-icon-box) * var(--badge-icon-inset-share), 1px)');
  });

  it('keeps every icon clear of the circle at every ui-scale', () => {
    const share = Number(iconRules.find(r => r.atRules === '')?.props.get('--badge-icon-inset-share'));
    expect(share).toBeGreaterThan(0);
    const badgeRem = parseFloat(decl(headerMarkCss, '--header-mark-count-size') ?? '');
    expect(badgeRem).toBeGreaterThan(0);
    for (const [icon, reach] of Object.entries(BADGE_ICON_REACH)) {
      for (let scale = UI_SCALE_MIN; scale <= UI_SCALE_MAX; scale += UI_SCALE_STEP) {
        const box = Math.floor(badgeRem * 16 * scale / 100);
        const inset = Math.floor(box * share);
        const size = box - 2 * inset;
        const clearance = box / 2 - reach * size;
        expect(inset, `${icon} at ${scale}%: a whole-pixel inset`).toBeGreaterThanOrEqual(1);
        expect(clearance / box, `${icon} at ${scale}% in a ${box}px box: ${clearance.toFixed(2)}px clear`)
          .toBeGreaterThanOrEqual(MIN_CLEARANCE_SHARE);
      }
    }
  });

  it('sizes the icon in badges.css alone, so no host overrides its room', () => {
    for (const path of styleSheetPaths(stylesDir)) {
      if (path.endsWith('/badges.css')) continue;
      const css: string = readFileSync(path, 'utf-8');
      const iconHosts = cssRules(css).filter(r => /\.brand-badge-(spinner|queued)\b/.test(r.selector));
      for (const rule of iconHosts) {
        expect(rule.props.has('width') || rule.props.has('height'), `${path}: ${rule.selector}`).toBe(false);
      }
      expect(css, path).not.toContain('--badge-icon-');
    }
  });

  it('checks clearance for every icon the brand badge can draw', () => {
    const badgeTsx: string = readFileSync(
      resolve(stylesDir, '..', 'components', 'layout', 'BrandBadge.tsx'), 'utf-8');
    const drawn = [...badgeTsx.matchAll(/class="(brand-badge-[a-z]+)"><([A-Z]\w*Icon) \/>/g)];
    expect(drawn.map(m => m[1]).sort()).toEqual(['brand-badge-queued', 'brand-badge-spinner']);
    expect(drawn.map(m => m[2]).sort()).toEqual(Object.keys(BADGE_ICON_REACH).sort());
  });
});

describe('the pill and the baseline sit on whole pixels', () => {

  it('rounds the design height to the whole pixel that splits most evenly', () => {
    expect(shared!.props.get('--badge-floor')).toBe('round(down, var(--badge-height), 1px)');
    expect(shared!.props.get('--badge-box')).toBe('clamp(var(--badge-floor) - 1px, '
      + 'round(1cap + 2 * round((var(--badge-floor) - 0.5px - 1cap) / 2, 1px), 1px), '
      + 'var(--badge-floor))');
  });

  it('never makes a pill taller than its design', () => {
    const box = shared!.props.get('--badge-box') ?? '';
    expect(box.endsWith(', var(--badge-floor))'), 'the clamp caps the box at the design').toBe(true);
    expect(shared!.props.get('height')).toBe('var(--badge-box)');
    expect(shared!.props.get('min-height')).toBe('0');
  });

  it('lifts the baseline by a whole-pixel bottom padding', () => {
    expect(shared!.props.get('--badge-baseline-inset'))
      .toBe('round((var(--badge-box) - 1cap) / 2, 1px)');
    expect(shared!.props.get('padding-block')).toBe('0 var(--badge-baseline-inset)');
  });

  it('has every badge with a pill state the height it was designed at', () => {
    const missing: string[] = [];
    const sheets: string[] = styleSheetPaths(stylesDir).map(p => readFileSync(p, 'utf-8'));
    for (const cls of GLYPH_BADGES) {
      const declared = sheets.some(css =>
        rulesTargeting(css, cls).some(r => r.props.has('--badge-height')));
      if (!declared) missing.push(cls);
    }
    expect(missing).toEqual([]);
  });

  it('keeps a single-digit badge round, with its width on the same box', () => {
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

  it('lets the dots keep the size their own rules give them', () => {
    for (const g of GLYPHLESS) {
      const reaching = rulesTargeting(badgesCss, g).filter(r => r.selector !== shared!.selector);
      expect(reaching.map(r => r.selector)).toEqual([]);
    }
  });
});

describe('the pill and the glyph share one grid across the badge', () => {
  const layer = cssRules(badgesCss).find(r => r.props.get('will-change') === 'transform');
  const width = cssRules(badgesCss).find(r => r.props.has('width'));
  const sheets = (): Array<{ path: string; css: string }> =>
    styleSheetPaths(resolve(stylesDir, '..'))
      .filter(p => !p.endsWith('badges.css'))
      .map(path => ({ path: path.split('/src/')[1], css: readFileSync(path, 'utf-8') }));

  it('puts every glyph badge on its own layer, ungated', () => {
    expect(layer, 'no will-change rule in badges.css').toBeDefined();
    expect(layer!.atRules).toBe('');
    expect(selectorList(layer!.selector)).toEqual(SHARED_SELECTORS);
  });

  it('rounds a content-sized pill UP to a whole-pixel width', () => {
    expect(width, 'no width rule in badges.css').toBeDefined();
    expect(width!.atRules).toBe('@supports (width: calc-size(fit-content, size))');
    expect(width!.props.get('width')).toBe('calc-size(fit-content, round(up, size, 1px))');
    expect(selectorList(width!.selector)).toEqual(SHARED_SELECTORS);
  });

  it('lets no other rule drop the layer', () => {
    const offenders: string[] = [];
    for (const { path, css } of sheets()) {
      for (const cls of GLYPH_BADGES) {
        for (const rule of rulesTargeting(css, cls)) {
          const hint = rule.props.get('will-change');
          if (hint !== undefined && !hint.split(/\s*,\s*/).includes('transform')) {
            offenders.push(`${path}: ${rule.selector} { will-change: ${hint} }`);
          }
        }
      }
    }
    expect(offenders).toEqual([]);
  });

  it('lets no badge state a width, only the min-width it is floored at', () => {
    const offenders: string[] = [];
    for (const { path, css } of sheets()) {
      for (const cls of GLYPH_BADGES) {
        for (const rule of rulesTargeting(css, cls)) {
          if (rule.props.has('width')) offenders.push(`${path}: ${rule.selector}`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});

describe('the ink, not the advance, sits in the middle', () => {
  const ink = cssRules(badgesCss).find(r => r.selector === '.badge-ink');

  it('moves the ink span by its measured shift, in paint only, ungated', () => {
    expect(ink, 'no .badge-ink rule in badges.css').toBeDefined();
    expect(ink!.atRules).toBe('');
    expect(ink!.props.get('position')).toBe('relative');
    expect(ink!.props.get('left')).toBe('var(--badge-ink-shift, 0em)');
  });

  it('renders every glyph badge through GlyphBadge, which states the shift', () => {
    const measured = GLYPH_BADGES.filter(c => c !== 'thread-status-question-badge');
    const offenders: string[] = [];
    for (const path of clientSourcePaths(resolve(stylesDir, '..'))) {
      if (path.endsWith('/GlyphBadge.tsx')) continue;
      const source: string = readFileSync(path, 'utf-8');
      for (const tag of source.matchAll(/<([a-z]+)\b[^>]*?\bclass=(?:"([^"]*)"|\{`([^`]*)`\})/g)) {
        const classes: string[] = (tag[2] ?? tag[3]).replace(/\$\{[^}]*\}/g, ' ').split(/\s+/);
        if (classes.some(c => measured.includes(c)) && !classes.some(c => [...GLYPHLESS, 'badge-icon'].includes(c))) {
          offenders.push(`${path.split('/src/')[1]}: <${tag[1]} class="${tag[2] ?? tag[3]}">`);
        }
      }
    }
    expect(offenders).toEqual([]);
  });
});

describe('nothing outranks it', () => {
  it('is the LAST stylesheet main.tsx imports', () => {
    const imports: string[] = [...mainTsx.matchAll(/^import '(\.\/styles\/[^']+)';$/gm)]
      .map(m => m[1]);
    expect(imports.length, 'no style imports found').toBeGreaterThan(1);
    expect(imports[imports.length - 1]).toBe('./styles/badges.css');
  });

  it('is the only place a badge is given a display or a box it cannot beat', () => {
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
            .filter(one => !GLYPHLESS.some(g => one.includes(`.${g}`)));
          if (beats.length) offenders.push(`${path.split('/src/')[1]}: ${beats.join(', ')}`);
        }
      }
    }
    expect(seen, 'the sweep found no badge box rules at all').toBeGreaterThanOrEqual(7);
    expect(offenders).toEqual([]);
  });
});

describe('the band the trim measures is the glyph the badge draws', () => {
  it('lets no badge put its font on a pseudo-element', () => {
    const pseudo = new RegExp(`\\.(${GLYPH_BADGES.join('|')})::?(before|after)\\b`);
    const offenders: string[] = [];
    let seen = 0;
    for (const path of styleSheetPaths(resolve(stylesDir, '..'))) {
      const css: string = readFileSync(path, 'utf-8');
      for (const rule of cssRules(css)) {
        for (const one of selectorList(rule.selector)) {
          if (!pseudo.test(one.split(/[\s>+~]+/).pop() ?? '')) continue;
          seen++;
          const fonts = [...rule.props.keys()].filter(p => /^font(-|$)/.test(p));
          if (fonts.length) offenders.push(`${path.split('/src/')[1]}: ${one} sets ${fonts.join(', ')}`);
        }
      }
    }
    expect(seen, 'the sweep found no badge pseudo-element rules at all').toBeGreaterThanOrEqual(2);
    expect(offenders).toEqual([]);
  });

  it('keeps the "?" glyph a step smaller than the badge it sits in', () => {
    const chatCss: string = readFileSync(resolve(stylesDir, 'chat', 'input-messages.css'), 'utf-8');
    const sized = rulesTargeting(chatCss, 'thread-status-question-badge')
      .filter(r => r.props.has('font-size'));
    expect(sized.map(r => r.props.get('font-size'))).toEqual(['var(--font-size-3xs)']);
  });
});

describe('a section label and its count share one cap band', () => {
  const sectionCss: string = readFileSync(resolve(stylesDir, 'section-header.css'), 'utf-8');
  const gated = cssRules(sectionCss).filter(r => r.atRules === '@supports (text-box-trim: trim-both)');

  it('gives the label the same trim the count now has', () => {
    const label = gated.find(r => r.selector === '.section-label');
    expect(label?.props.get('text-box-trim')).toBe('trim-both');
    expect(label?.props.get('text-box-edge')).toBe('cap alphabetic');
    expect(label?.props.get('align-content')).toBe('center');
  });

  it('holds the label at its untrimmed box, which two other things ride on', () => {
    const label = gated.find(r => r.selector === '.section-label');
    expect(label?.props.get('min-height')).toBe('1lh');
  });

  it('retires the hand-tuned lift wherever the trim lands, and only there', () => {
    const retired = gated.find(r =>
      r.selector === '.list-section-title .section-count');
    expect(retired?.props.get('top')).toBe('0');
    const base = cssRules(sectionCss).find(r =>
      r.selector === '.list-section-title .section-count'
      && r.atRules === '');
    expect(base?.props.get('top'), 'the untrimmed fallback keeps its lift').toBe('-0.0625rem');
  });
});
