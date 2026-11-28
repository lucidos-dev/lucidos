import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl, rulesTargeting } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentsCss = readFileSync(resolve(here, '../components.css'), 'utf-8');

const ROOT_SIZES: number[] = [];
for (let scale = 75; scale <= 200; scale += 12.5) ROOT_SIZES.push((16 * scale) / 100);

const RINGED = ['image-popup-close', 'image-popup-copy', 'image-popup-nav', 'image-popup-zoom', 'floating-mobile-close'];

const RING_PX = 2;

function px(value: string, root: number): number {
  const capped = value.match(/^min\((.+), var\(--radius-round\)\)$/);
  if (capped) return px(capped[1], root);
  const calc = value.match(/^calc\((.+)\)$/);
  if (calc) return calc[1].split('+').reduce((sum, term) => sum + px(term.trim(), root), 0);
  if (value === 'var(--image-popup-ring)') return RING_PX;
  const rem = value.match(/^(-?[\d.]+)rem$/);
  if (rem) return Number(rem[1]) * root;
  const raw = value.match(/^(-?[\d.]+)px$/);
  if (raw) return Number(raw[1]);
  throw new Error(`cannot resolve "${value}" to px`);
}

const closeRule = block(componentsCss, '.image-popup-close,\n.image-popup-copy {');
const zoomRule = block(componentsCss, '.image-popup-zoom {');
const zoomBtnRule = block(componentsCss, '.image-popup-zoom-btn {');
const zoomLevelRule = block(componentsCss, '.image-popup-zoom-level {');

function pillBox(root: number): { width: number; height: number } {
  const pad = px(decl(zoomRule, 'padding')!, root);
  const gap = px(decl(zoomRule, 'gap')!, root);
  const ring = px(decl(zoomRule, 'border')!.split(' ')[0], root);
  const btn = px(decl(zoomBtnRule, 'min-width')!, root);
  const level = px(decl(zoomLevelRule, 'min-width')!, root);
  const btnHeight = px(decl(zoomBtnRule, 'height')!, root);
  return {
    width: 2 * ring + 2 * pad + 2 * gap + 2 * btn + level,
    height: 2 * ring + 2 * pad + btnHeight,
  };
}

describe('the image popup draws even hairlines', () => {
  it('widens the ring to two device pixels below 2dppx', () => {
    expect(decl(block(componentsCss, '.image-popup {'), '--image-popup-ring')).toBe(`${RING_PX}px`);

    const prelude = '@media (min-resolution: 2dppx), (-webkit-min-device-pixel-ratio: 2)';
    expect(componentsCss, 'the retina arm needs both spellings').toContain(prelude);
    expect(decl(block(componentsCss, prelude), '--image-popup-ring')).toBe('1px');
  });

  it('draws every ringed control at that width, never a literal one', () => {
    const literal = RINGED.flatMap((cls) =>
      rulesTargeting(componentsCss, cls)
        .filter((rule) => rule.props.has('border'))
        .filter((rule) => !rule.props.get('border')!.startsWith('var(--image-popup-ring)'))
        .map((rule) => `${rule.atRules} ${rule.selector} { border: ${rule.props.get('border')} }`),
    );

    expect(literal, 'a fixed border width leaves this ring ragged on a 1x display').toEqual([]);
  });

  it('keeps the pill a true stadium by deriving its radius from its own height', () => {
    const radius = decl(zoomRule, 'border-radius')!;
    for (const root of ROOT_SIZES) {
      expect(px(radius, root), `radius at root ${root}px`).toBe(pillBox(root).height / 2);
    }
  });

  it('lands both boxes on whole pixels at every UI scale', () => {
    const fractional: string[] = [];
    for (const root of ROOT_SIZES) {
      const close = px(decl(closeRule, 'width')!, root);
      expect(px(decl(closeRule, 'height')!, root), 'the close button is a circle').toBe(close);
      const pill = pillBox(root);
      const boxes: [string, number][] = [
        ['close', close],
        ['pill width', pill.width],
        ['pill height', pill.height],
      ];
      for (const [what, value] of boxes) {
        if (!Number.isInteger(value)) fractional.push(`root ${root}px: ${what} = ${value}px`);
      }
    }

    expect(fractional, 'a hairline around a fractional box paints unevenly').toEqual([]);
  });

  it('sizes the close button itself rather than inheriting .icon-btn padding', () => {
    expect(decl(closeRule, 'padding')).toBe('0');
    expect(decl(closeRule, 'width')).not.toBeNull();
    expect(decl(closeRule, 'height')).not.toBeNull();
  });
});
