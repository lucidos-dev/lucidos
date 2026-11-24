import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error same
import { dirname, resolve } from 'node:path';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';

import { NAV_FOCUS_FADE_MS, NAV_FOCUS_RAMP_MS } from '../focusMarker';

const here: string = dirname(fileURLToPath(import.meta.url));
const stripComments = (source: string): string => source.replace(/\/\*[\s\S]*?\*\//g, '');
const css = stripComments(
  readFileSync(resolve(here, '../../../styles/global/host-components.css'), 'utf-8'),
);
const baseCss = stripComments(
  readFileSync(resolve(here, '../../../styles/global/base.css'), 'utf-8'),
);

function block(selector: string): string {
  const rules = [...css.matchAll(/^([^\s{}@][^{}]*?)\{([^}]*)\}/gm)];
  return rules
    .filter(m => m[1].split(',').map(part => part.trim()).includes(selector))
    .map(m => m[2])
    .join('\n');
}

function declaration(source: string, property: string): string | undefined {
  const escaped = property.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return source.match(new RegExp(`(?:^|;)\\s*${escaped}\\s*:\\s*([^;]+)`, 'm'))?.[1].trim();
}

function layers(shadow: string | undefined): string[] {
  const out: string[] = [];
  let depth = 0;
  let current = '';
  for (const ch of shadow ?? '') {
    if (ch === '(') depth++;
    else if (ch === ')') depth--;
    if (ch === ',' && depth === 0) {
      out.push(current.trim());
      current = '';
      continue;
    }
    current += ch;
  }
  if (current.trim()) out.push(current.trim());
  return out;
}

describe('nav focus marker paint: background highlight, not a frame', () => {
  it('fills the box and glows past it, all on one box-shadow', () => {
    const stuck = block('.nav-focus-stuck');
    expect(stuck).not.toBe('');
    const shadow = declaration(stuck, 'box-shadow');
    expect(shadow).toBeDefined();
    expect(shadow).toMatch(/inset\s+0\s+0\s+0\s+100vmax/);
    const outerLayers = layers(shadow).filter(l => !l.includes('inset'));
    const glowLayers = outerLayers.filter(l => /^0\s+0\s+[\d.]+rem\s/.test(l));
    expect(glowLayers.length).toBeGreaterThanOrEqual(2);
    expect(outerLayers.length).toBe(glowLayers.length + 1);
  });

  it('draws a hairline border whose strength each theme sets', () => {
    const shadow = declaration(block('.nav-focus-stuck'), 'box-shadow');
    const border = layers(shadow).filter(l => /^0\s+0\s+0\s+1px\s/.test(l));
    expect(border).toHaveLength(1);
    expect(border[0]).toContain('var(--nav-focus-border-alpha)');
  });

  it('paints no visible frame', () => {
    const stuck = block('.nav-focus-stuck');
    expect(declaration(stuck, 'border')).toBeUndefined();
    const outline = declaration(stuck, 'outline');
    expect(outline).toBe('0.125rem solid transparent');
    expect(declaration(stuck, 'background')).toBeUndefined();
    expect(declaration(stuck, 'background-color')).toBeUndefined();
    expect(declaration(stuck, 'background-image')).toBeUndefined();
  });

  it('survives forced-colors mode via the transparent outline', () => {
    expect(declaration(block('.nav-focus-stuck'), 'outline')).toContain('transparent');
  });

  it('derives every colour from --nav-focus-glow, never from --accent', () => {
    const stuck = block('.nav-focus-stuck');
    const colours = [...stuck.matchAll(/color-mix\([^)]*\)/g)].map(m => m[0]);
    expect(colours.length).toBeGreaterThan(0);
    expect(colours.every(c => c.includes('var(--nav-focus-glow)'))).toBe(true);
    expect(stuck).not.toMatch(/var\(--accent[-)]/);
    expect(stuck).not.toMatch(/#[0-9a-fA-F]{3,8}\b/);
  });

  it('has that token defined by BOTH themes, since a missing one erases the marker', () => {
    const themeBlock = (selector: string): string => {
      const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
      const body = baseCss.match(new RegExp(`^${escaped}\\s*\\{([^}]*)\\}`, 'm'))?.[1];
      expect(body, `no top-level \`${selector}\` block in base.css`).toBeDefined();
      return body ?? '';
    };
    for (const selector of ['html, html[data-theme-mode="dark"]', 'html[data-theme-mode="light"]']) {
      expect(declaration(themeBlock(selector), '--nav-focus-glow')).toMatch(
        /^#[0-9a-fA-F]{3,8}$/,
      );
      expect(declaration(themeBlock(selector), '--nav-focus-border-alpha')).toMatch(/^\d+%$/);
    }
    expect(declaration(themeBlock('html, html[data-theme-mode="dark"]'), '--nav-focus-border-alpha')).toBe(
      '0%',
    );
  });

  it('turns on by rising to full and STOPPING there, with nothing to sag back from', () => {
    const stuck = block('.nav-focus-stuck');
    const animation = declaration(stuck, 'animation');
    expect(animation).toMatch(/^nav-focus-spotlight-on\s/);
    expect(Number(animation?.match(/([\d.]+)s/)?.[1]) * 1000).toBe(NAV_FOCUS_RAMP_MS);
    const keyframes = css.match(/@keyframes\s+nav-focus-spotlight-on\s*\{([\s\S]*?)\n\}/)?.[1];
    expect(keyframes).toBeDefined();

    const zeroStop = keyframes?.match(/0%\s*\{([^}]*)\}/)?.[1] ?? '';
    expect(zeroStop).not.toBe('');
    expect(layers(declaration(zeroStop, 'box-shadow')).length).toBe(4);
    expect(layers(declaration(zeroStop, 'box-shadow')).every(l => l.includes('transparent'))).toBe(
      true,
    );

    expect(keyframes?.match(/\d+%\s*\{|\b(?:from|to)\s*\{/g)).toEqual(['0% {']);
    expect(keyframes).not.toMatch(/color-mix/);

    expect(declaration(stuck, 'animation-fill-mode')).toBeUndefined();
    expect(animation).not.toMatch(/forwards|both/);

    const restingShadow = declaration(stuck, 'box-shadow');
    const insets = (shadow: string | undefined) =>
      layers(shadow).map(l => l.includes('inset')).join(',');
    expect(layers(declaration(zeroStop, 'box-shadow')).length).toBe(layers(restingShadow).length);
    expect(insets(declaration(zeroStop, 'box-shadow'))).toBe(insets(restingShadow));
  });

  it('dissolves via an ANIMATION so it still runs when it interrupts the turn-on', () => {
    const fading = block('.nav-focus-stuck.nav-focus-fading');
    expect(fading).not.toBe('');

    expect(declaration(fading, 'transition')).toBeUndefined();
    const animation = declaration(fading, 'animation');
    expect(animation).toMatch(/^nav-focus-spotlight-off\s/);
    expect(animation).toMatch(/\bforwards\b/);
    expect(animation).toMatch(/\blinear\b/);

    const off = css.match(/@keyframes\s+nav-focus-spotlight-off\s*\{([\s\S]*?)\n\}/)?.[1];
    expect(off).toBeDefined();
    expect(off).not.toMatch(/0%\s*\{|\bfrom\s*\{/);
    const toStop = off?.match(/(?:to|100%)\s*\{([^}]*)\}/)?.[1] ?? '';
    const endShadow = declaration(toStop, 'box-shadow');
    const restingShadow = declaration(block('.nav-focus-stuck'), 'box-shadow');
    expect(layers(endShadow).length).toBe(layers(restingShadow).length);
    expect(layers(endShadow).map(l => l.includes('inset')).join(',')).toBe(
      layers(restingShadow).map(l => l.includes('inset')).join(','),
    );
    expect(layers(endShadow).every(l => l.includes('transparent'))).toBe(true);

    const seconds = Number(animation?.match(/([\d.]+)s/)?.[1]);
    expect(seconds * 1000).toBe(NAV_FOCUS_FADE_MS);
    const onSeconds = Number(
      declaration(block('.nav-focus-stuck'), 'animation')?.match(/([\d.]+)s/)?.[1],
    );
    expect(seconds).toBeGreaterThan(onSeconds);
    expect(seconds).toBeLessThanOrEqual(1);
  });

  it('drops both the turn-on and the dissolve under reduced motion', () => {
    const reduce = ':root[data-motion="reduce"] ';
    expect(declaration(block(`${reduce}.nav-focus-stuck`), 'animation')).toBe('none');
    expect(declaration(block(`${reduce}.nav-focus-stuck.nav-focus-fading`), 'animation')).toBe('none');
    expect(css).not.toMatch(/prefers-reduced-motion/);
  });

  it('rides the Animation speed slider on both the turn-on and the dissolve', () => {
    expect(declaration(block('.nav-focus-stuck'), 'animation'))
      .toMatch(/calc\(0\.45s \* var\(--duration-scale\)\)/);
    expect(declaration(block('.nav-focus-stuck.nav-focus-fading'), 'animation'))
      .toMatch(/calc\(0\.8s \* var\(--duration-scale\)\)/);
  });
});

describe('arrival marker paint: the nav marker light, from the same rules', () => {
  const reduce = ':root[data-motion="reduce"] ';
  const pairs: [string, string][] = [
    ['.nav-focus-stuck', '.arrival-marker'],
    ['.nav-focus-stuck.nav-focus-fading', '.arrival-marker.arrival-marker-fading'],
    [`${reduce}.nav-focus-stuck`, `${reduce}.arrival-marker`],
    [`${reduce}.nav-focus-stuck.nav-focus-fading`, `${reduce}.arrival-marker.arrival-marker-fading`],
  ];

  it.each(pairs)('%s and %s share one rule', (nav, arrival) => {
    expect(block(arrival)).not.toBe('');
    expect(block(arrival)).toBe(block(nav));
  });

  it('writes the wash once', () => {
    const outsideKeyframes = css.replace(/@keyframes[^{]*\{[\s\S]*?\n\}/g, '');
    expect(outsideKeyframes.match(/inset 0 0 0 100vmax/g)).toHaveLength(1);
  });
});
