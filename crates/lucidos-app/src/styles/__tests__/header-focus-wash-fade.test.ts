import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { cssRules, rulesTargeting, selectorList, type CssRule } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const stylesDir: string = resolve(here, '..');
const shellCss: string = readFileSync(resolve(stylesDir, 'panels/shell.css'), 'utf-8');

const WASH = 'header-focus-wash';

function tintRules(): CssRule[] {
  return cssRules(shellCss).filter(
    (r) => r.selector.includes(`.${WASH}`) && r.selector.includes('::before'),
  );
}

function baseTintRule(): CssRule {
  const found = tintRules().filter(r => r.selector === `.${WASH}::before`);
  expect(found.length, `expected exactly one base \`.${WASH}::before\` rule`).toBe(1);
  return found[0];
}

describe('the wash fades a colour, so no compositing layer is conjured', () => {
  it('states no opacity on the box, in any state', () => {
    for (const rule of rulesTargeting(shellCss, WASH)) {
      expect(rule.props.get('opacity'), `${rule.selector} fades the box again`).toBeUndefined();
    }
  });

  it('transitions nothing but its geometry on the box', () => {
    for (const rule of rulesTargeting(shellCss, WASH)) {
      const transition = rule.props.get('transition');
      if (!transition) continue;
      expect(transition, `${rule.selector} animates an accelerated property`)
        .not.toMatch(/opacity|transform|filter/);
    }
  });

  it('rests transparent and fades background-color instead', () => {
    expect(baseTintRule().props.get('background-color')).toBe('transparent');
    expect(baseTintRule().props.get('transition')).toMatch(/^background-color /);
  });

  it('scales the fade by the animation-speed slider', () => {
    expect(baseTintRule().props.get('transition')).toContain('var(--duration-scale)');
  });

  it('reveals the focused pane by painting the tint, one rule for the three', () => {
    const reveal = tintRules().filter(
      r => r.props.get('background-color') === 'var(--focus-header-tint)',
    );
    expect(reveal.length, 'the tint is painted from more than one rule').toBe(1);
    for (const pane of ['drawer', 'thread', 'content']) {
      expect(reveal[0].selector).toContain(`[data-pane="${pane}"]::before`);
    }
  });

  it('stops the underline at the pane edge, never over a divider', () => {
    for (const rule of tintRules()) {
      expect(rule.props.get('box-shadow'), `${rule.selector} draws the underline over a divider`)
        .toBeUndefined();
    }
    const lineRules = cssRules(shellCss).filter(
      r => r.selector.includes(`.${WASH}`) && r.selector.includes('::after'),
    );
    const base = lineRules.filter(r => r.selector === `.${WASH}::after`);
    expect(base.length, `expected exactly one base \`.${WASH}::after\` rule`).toBe(1);
    expect(base[0].props.get('inset')).toBe('0');
    for (const rule of lineRules) {
      for (const edge of ['left', 'right', 'inset']) {
        const value = rule.props.get(edge);
        if (value !== undefined) expect(value, `${rule.selector} moves the line off the box`).toBe('0');
      }
    }
    const reveal = lineRules.filter(r => r.props.get('box-shadow')?.includes('var(--focus-header-underline)'));
    expect(reveal.length, 'the underline is painted from more than one rule').toBe(1);
    for (const pane of ['drawer', 'thread', 'content']) {
      expect(reveal[0].selector).toContain(`[data-pane="${pane}"]::after`);
    }
  });

  it('keeps the box, and only the box, on the pane-resize kill list', () => {
    const killed = cssRules(shellCss)
      .filter(r => r.selector.includes('data-pane-resizing'))
      .flatMap(r => selectorList(r.selector))
      .filter(one => one.includes(WASH));
    expect(killed, 'the wash left the pane-resize kill list')
      .toEqual([`:root[data-pane-resizing] .${WASH}`]);
  });
});
