/**
 * Source scans over the connection bar's stylesheet promises. Each is a property
 * of the RULE rather than of a rendered frame, so each is cheaper and sharper to
 * assert here than in a browser.
 *
 * 1. Every quantity is a custom property declared on `:root`. The live style
 *    remote retunes by writing custom properties inline on <html>, which beats a
 *    `:root` rule; a number written straight into a rule is out of its reach,
 *    and a `var()` naming a property nothing declares silently resolves to
 *    nothing at all.
 * 2. The dot takes its colour from the shared `.status-dot` scale and only its
 *    SIZE from here, so a state's colour is stated once per state in the app
 *    rather than once per element.
 * 3. No coloured left edge, on either bar. Banned outright by
 *    `.claude/rules/frontend-css.md`.
 * 4. No motion. The bar's arrival is the event; a wash pulsing under a sentence
 *    the user is reading is noise, and the absence is what makes a
 *    prefers-reduced-motion guard unnecessary rather than forgotten.
 * 5. The dot's first-line offset is derived from the two quantities that decide
 *    it, both of them the ones actually in force, or it drifts off the line the
 *    moment either is retuned. Same derivation as the Lucidos menu's notice.
 * 6. On mobile the bar spans the window. It is a child of the fixed header
 *    there, so it is full-bleed only while that header keeps its inline inset
 *    on its rows. A column-level inset reaches the bar too.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, cssRules, decl, rulesTargeting, selectorList } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const styles = (rel: string): string => readFileSync(resolve(stylesDir, rel), 'utf-8');

const shellCss = styles('panels/shell.css');
const baseCss = styles('global/base.css');
const mobileCss = styles('mobile.css');
const rootTokens = block(baseCss, ':root');

/** The three sections the mobile header swaps between, one per pane. */
const MOBILE_HEADER_SECTIONS = [
  '.mobile-threads-header',
  '.mobile-thread-header',
  '.mobile-content-header',
];

/** Every property that can inset a box horizontally. The scan reads all of
 *  them, so a shorthand or a single side cannot walk around a guard written
 *  for `padding-inline` alone. */
const INLINE_PADDING_PROPS = [
  'padding',
  'padding-inline',
  'padding-inline-start',
  'padding-inline-end',
  'padding-left',
  'padding-right',
];

/** How the mobile breakpoint insets the element carrying `className`, as
 *  `prop: value` pairs in source order. Empty when nothing there pads it. */
function mobileInlinePadding(className: string): string[] {
  return rulesTargeting(mobileCss, className)
    .filter(rule => rule.atRules.includes('max-width: 768px'))
    .flatMap(rule => INLINE_PADDING_PROPS
      .filter(prop => rule.props.has(prop))
      .map(prop => `${prop}: ${rule.props.get(prop)}`));
}

/** Every rule the bar is made of, its inner parts included. `rulesTargeting`
 *  answers "which rules style THIS element", which deliberately drops
 *  `.connection-banner .status-dot`; the quantities live in those descendant
 *  rules as much as in the bar's own, so the scan wants both. */
const barRules = cssRules(shellCss).filter(r => r.selector.includes('connection-banner'));

/** Every custom property base.css declares on the document root, whichever host
 *  it uses: the `:root` tokens block, and the `html` / `html[data-theme=…]`
 *  blocks the themed colours live on (a theme has to out-specify `:root`, which
 *  is why the colours are not in it). */
const rootDeclared = new Set<string>(
  cssRules(baseCss)
    .filter(r => r.selector.split(',').some(s => /^\s*(:root|html)\b/.test(s)))
    .flatMap(r => [...r.props.keys()].filter(p => p.startsWith('--'))),
);

describe('the connection bar is tunable from :root', () => {
  it('declares every quantity it reads', () => {
    // A `var(--gone)` reads as nothing and the rule silently loses the
    // declaration, which looks exactly like a rule nobody wrote.
    const named = new Set<string>();
    for (const rule of barRules) {
      for (const match of rule.body.matchAll(/var\((--[\w-]+)/g)) named.add(match[1]);
    }
    expect(named.size, 'the bar reads no custom properties at all').toBeGreaterThan(0);
    for (const name of named) {
      expect(rootDeclared, `${name} is read by the bar but declared on no document root`)
        .toContain(name);
    }
  });

  it('keeps its own tunables in the :root token block, where the remote finds them', () => {
    // The themed colours are a different family and live on the theme hosts.
    // These are the bar's own numbers, and a number the remote cannot reach is
    // the one thing `.claude/rules/frontend-css.md` says this file must not do.
    for (const name of ['--banner-tint-strength', '--banner-line-height', '--connection-banner-dot']) {
      expect(decl(rootTokens, name), `${name} is not on :root`).not.toBeNull();
    }
  });

  it('writes no bare length or percentage into a rule', () => {
    for (const rule of barRules) {
      for (const line of rule.body.split('; ')) {
        const [property, ...rest] = line.split(':');
        const value = rest.join(':');
        if (!value || property.trim().startsWith('--')) continue;
        // Borders are exempt at any width (`.claude/rules/frontend-css.md`: a
        // border width is snapped to a whole unit before layout).
        if (property.trim().startsWith('border') && /\dpx/.test(value)) continue;
        // `1em` is not a quantity, it is "this element's own font size", which
        // is what makes the dot's offset measure the line box beside it. Same
        // literal, for the same reason, in the Lucidos menu's notice.
        const tunable = value.replace(/\b1em\b/g, '');
        expect(tunable, `${rule.selector} hardcodes ${line.trim()}, out of the style remote's reach`)
          .not.toMatch(/\d+(\.\d+)?(rem|em|px|%)/);
      }
    }
  });
});

describe('the bar states each thing once', () => {
  it('sizes the dot but leaves its colour to the shared scale', () => {
    const dot = block(shellCss, '.connection-banner .status-dot {');
    expect(decl(dot, 'width')).toBe('var(--connection-banner-dot)');
    expect(decl(dot, 'background'), 'the dot restates a colour the scale already gives it')
      .toBeNull();
  });

  it('lands the dot on the first line, derived rather than nudged', () => {
    // The sentence wraps at a narrow split, and a dot centred against the whole
    // box floats into the middle of it. Both quantities the offset is made of
    // have to be the ones actually in force.
    const dot = block(shellCss, '.connection-banner .status-dot {');
    const offset = decl(dot, 'margin-top') ?? '';
    expect(offset).toContain('var(--banner-line-height)');
    expect(offset).toContain('var(--connection-banner-dot)');
    expect(decl(dot, 'align-self')).toBe('flex-start');
    // The bar states the line-height the offset is derived from, rather than
    // inheriting a value this file cannot name. Read through the parsed rules
    // rather than by textual match: the bar's first `.connection-banner {` is a
    // member of the grouped rule it shares with the backup reminder, which is
    // deliberately NOT where this lives.
    const own = barRules.find(r => r.selector === '.connection-banner');
    expect(own?.props.get('line-height')).toBe('var(--banner-line-height)');
  });

  it('washes each degraded state in the colour its dot already carries', () => {
    const wash = (status: string) =>
      decl(block(shellCss, `.connection-banner[data-conn='${status}'] {`), 'background') ?? '';
    expect(wash('disconnected')).toContain('var(--accent-red)');
    expect(wash('connecting')).toContain('var(--accent-yellow)');
    for (const status of ['disconnected', 'connecting']) {
      expect(wash(status), 'the wash strength is a shared token, not a per-bar guess')
        .toContain('var(--banner-tint-strength)');
    }
  });
});

describe('neither bar reaches for a banned or unguarded device', () => {
  it('carries no coloured left edge', () => {
    for (const rule of barRules.concat(rulesTargeting(shellCss, 'backup-reminder'))) {
      expect(rule.body, `${rule.selector} paints a left edge`).not.toMatch(/border-left\s*:/);
      expect(rule.selector, `${rule.selector} fakes a left bar`).not.toMatch(/::before|::after/);
    }
  });

  it('animates nothing, so there is no motion to guard', () => {
    for (const rule of barRules) {
      expect(rule.body, `${rule.selector} animates`).not.toMatch(/animation\s*:|transition\s*:/);
    }
  });
});

describe('the mobile header hands its inline inset to its rows', () => {
  // Both bars mount inside that header on this viewport, so the inset the
  // desktop bar's controls want reaches them too. It left each bar floating a
  // glyph's width inside both screen edges, its border stopping short of each.
  it('leaves the column itself unpadded, so a bar spans the window', () => {
    expect(mobileInlinePadding('app-header'), 'the mobile header pads its own column')
      .toEqual(['padding-inline: 0']);
  });

  it('pads each pane section instead, which is the row the inset was for', () => {
    for (const section of MOBILE_HEADER_SECTIONS) {
      expect(mobileInlinePadding(section.slice(1)), `${section} takes the wrong inset`)
        .toEqual(['padding-inline: var(--header-padding-x)']);
    }
    // One grouped rule, so the three cannot drift apart.
    const grouped = rulesTargeting(mobileCss, 'mobile-thread-header')
      .find(rule => rule.props.has('padding-inline'));
    expect(selectorList(grouped!.selector)).toEqual(MOBILE_HEADER_SECTIONS);
  });

  it('leaves the row itself unpadded, so every centred cluster keeps its box', () => {
    // The clusters are absolutely positioned against the row, and their
    // non-overlap reserves are derived from its width. An inset moved onto the
    // row would change both.
    expect(mobileInlinePadding('mobile-header-row'), 'the row now pads itself too')
      .toEqual([]);
  });
});
