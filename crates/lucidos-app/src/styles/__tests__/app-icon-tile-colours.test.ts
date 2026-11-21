/**
 * The app icon tile paints only theme variables, so every theme recolours it
 * (ADR 0414, invariant I4). `AppIcon` picks the monogram hue from
 * `MONOGRAM_HUES`, which its own test holds to theme variables.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import { block, decl, cssRules } from './css-rule-helpers';

const css = readFileSync(new URL('../global/host-components.css', import.meta.url), 'utf8');
const COLOUR_PROPS = ['color', 'background', 'background-color', 'border', 'border-color', 'box-shadow'];

describe('the app icon tile', () => {
  it('overlays a rounded theme hairline without consuming image space or pointer events', () => {
    const tile = block(css, '.app-icon {');
    const ring = block(css, '.app-icon::after {');
    expect(decl(tile, 'position')).toBe('relative');
    expect(decl(ring, 'content')).toBe('""');
    expect(decl(ring, 'position')).toBe('absolute');
    expect(decl(ring, 'inset')).toBe('0');
    expect(decl(ring, 'border-radius')).toBe('inherit');
    expect(decl(ring, 'box-shadow')).toBe('inset 0 0 0 1px var(--border-color)');
    expect(decl(ring, 'pointer-events')).toBe('none');
    expect(decl(tile, 'border')).toBeNull();
    expect(decl(tile, 'padding')).toBeNull();
  });

  it('paints with theme variables only', () => {
    const rules = cssRules(css).filter((rule) => rule.selector.includes('.app-icon'));
    expect(rules.length).toBeGreaterThan(0);
    for (const rule of rules) {
      for (const prop of COLOUR_PROPS) {
        const value = rule.props.get(prop);
        if (value === undefined || value === 'none') continue;
        expect(value, `${rule.selector} ${prop}`).not.toMatch(/#[0-9a-f]{3,8}\b|rgba?\(|hsla?\(/i);
        expect(value, `${rule.selector} ${prop}`).toMatch(/var\(--/);
      }
    }
  });
});
