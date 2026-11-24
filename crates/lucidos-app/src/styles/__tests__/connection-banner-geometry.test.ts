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

const MOBILE_HEADER_SECTIONS = [
  '.mobile-threads-header',
  '.mobile-thread-header',
  '.mobile-content-header',
];

const INLINE_PADDING_PROPS = [
  'padding',
  'padding-inline',
  'padding-inline-start',
  'padding-inline-end',
  'padding-left',
  'padding-right',
];

function mobileInlinePadding(className: string): string[] {
  return rulesTargeting(mobileCss, className)
    .filter(rule => rule.atRules.includes('(--phone-layout)'))
    .flatMap(rule => INLINE_PADDING_PROPS
      .filter(prop => rule.props.has(prop))
      .map(prop => `${prop}: ${rule.props.get(prop)}`));
}

const barRules = cssRules(shellCss).filter(r => r.selector.includes('connection-banner'));

const rootDeclared = new Set<string>(
  cssRules(baseCss)
    .filter(r => r.selector.split(',').some(s => /^\s*(:root|html)\b/.test(s)))
    .flatMap(r => [...r.props.keys()].filter(p => p.startsWith('--'))),
);

describe('the connection bar is tunable from :root', () => {
  it('declares every quantity it reads', () => {
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
        if (property.trim().startsWith('border') && /\dpx/.test(value)) continue;
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
    const dot = block(shellCss, '.connection-banner .status-dot {');
    const offset = decl(dot, 'margin-top') ?? '';
    expect(offset).toContain('var(--banner-line-height)');
    expect(offset).toContain('var(--connection-banner-dot)');
    expect(decl(dot, 'align-self')).toBe('flex-start');
    const own = barRules.find(r => r.selector === '.connection-banner');
    expect(own?.props.get('line-height')).toBe('var(--banner-line-height)');
  });

  it('washes each degraded state in the colour its dot already carries', () => {
    const wash = (selector: string) => decl(block(shellCss, `${selector} {`), 'background') ?? '';
    const washes: Record<string, string> = {
      disconnected: wash(`.connection-banner[data-conn='disconnected']`),
      connecting: wash(`.connection-banner[data-conn='connecting'],\n.connection-banner[data-conn='stopped'],\n.connection-banner[data-conn='starting'],\n.connection-banner[data-conn='restarting'],\n.connection-banner[data-conn='switching-version']`),
    };
    expect(washes.disconnected).toContain('var(--accent-red)');
    expect(washes.connecting).toContain('var(--accent-yellow)');
    for (const [status, value] of Object.entries(washes)) {
      expect(value, `${status}: the wash strength is a shared token, not a per-bar guess`)
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
  it('leaves the column itself unpadded, so a bar spans the window', () => {
    expect(mobileInlinePadding('app-header'), 'the mobile header pads its own column')
      .toEqual(['padding-inline: 0']);
  });

  it('pads each pane section instead, which is the row the inset was for', () => {
    for (const section of MOBILE_HEADER_SECTIONS) {
      expect(mobileInlinePadding(section.slice(1)), `${section} takes the wrong inset`)
        .toEqual(['padding-inline: calc(var(--header-padding-x) + var(--phone-side-inset))']);
    }
    const grouped = rulesTargeting(mobileCss, 'mobile-thread-header')
      .find(rule => rule.props.has('padding-inline'));
    expect(selectorList(grouped!.selector)).toEqual(MOBILE_HEADER_SECTIONS);
  });

  it('leaves the row itself unpadded, so every centred cluster keeps its box', () => {
    expect(mobileInlinePadding('mobile-header-row'), 'the row now pads itself too')
      .toEqual([]);
  });
});
