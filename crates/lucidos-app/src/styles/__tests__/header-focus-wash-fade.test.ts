/**
 * The focused-pane wash fades a COLOUR, never an opacity.
 *
 * The bug: on the packaged macOS build the drawer's "Threads" and the Canvas
 * pane's title stepped up as a pane took focus. Both dropped back when the fade
 * finished. Each is a centred box over its pane's header segment, which is the
 * area the wash covers.
 *
 * The wash used to fade `opacity: 0` to `1`. An engine animates opacity on the
 * compositor, so that transition conjured a layer under the segment and tore it
 * down at the end. Safari re-rasterises whatever overlaps such a layer, and
 * re-rasterised text lands on a different device pixel. Two structural changes
 * per focus shift, hence up and back down. Same defect as the brand mark's press
 * cue (styles/header-mark.css): a change of KIND, not of value. That one was
 * cured with a permanent layer, this one the cheaper way round, by fading a
 * property no engine accelerates.
 *
 * A SOURCE SCAN because no WebDriver reaches WKWebView (ADR 0016).
 */
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

/** Every rule whose subject is the wash's ::before, which `rulesTargeting`
 *  deliberately drops (its subject is a pseudo-element, not the box). */
function tintRules(): CssRule[] {
  return cssRules(shellCss).filter(
    (r) => r.selector.includes(`.${WASH}`) && r.selector.includes('::before'),
  );
}

/** The unqualified `::before`, which carries the resting colour and the fade. */
function baseTintRule(): CssRule {
  const found = tintRules().filter(r => r.selector === `.${WASH}::before`);
  expect(found.length, `expected exactly one base \`.${WASH}::before\` rule`).toBe(1);
  return found[0];
}

describe('the wash fades a colour, so no compositing layer is conjured', () => {
  it('states no opacity on the box, in any state', () => {
    // Every rule for this box is in scope. An `opacity` anywhere on it brings
    // the accelerated property back, whichever rule carries it.
    for (const rule of rulesTargeting(shellCss, WASH)) {
      expect(rule.props.get('opacity'), `${rule.selector} fades the box again`).toBeUndefined();
    }
  });

  it('transitions nothing but its geometry on the box', () => {
    // Left/width are layout, and they exist for a divider drag rather than for
    // focus. Naming opacity, transform or filter here would put the fade back
    // on the compositor.
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
    // The duration is a literal rather than a token, being slower than
    // --duration-slow on purpose. So it carries the scale itself, or it is the
    // one animation in the app that ignores the setting.
    expect(baseTintRule().props.get('transition')).toContain('var(--duration-scale)');
  });

  it('reveals the focused pane by painting the tint, one rule for the three', () => {
    // One grouped rule, so the three panes cannot drift into three fades. Its
    // members are the ::before, not the box. A background on the box would stop
    // at the box edges, leaving the divider seams unwashed.
    const reveal = tintRules().filter(
      r => r.props.get('background-color') === 'var(--focus-header-tint)',
    );
    expect(reveal.length, 'the tint is painted from more than one rule').toBe(1);
    for (const pane of ['drawer', 'thread', 'content']) {
      expect(reveal[0].selector).toContain(`[data-pane="${pane}"]::before`);
    }
  });

  it('keeps the box, and only the box, on the pane-resize kill list', () => {
    // The kill rule switches transitions off so the header tracks the pointer
    // 1:1 during a divider drag. It must reach the geometry and leave the fade
    // alone. Focus does not change mid-drag, so killing the fade there would
    // only make a later shift pop.
    const killed = cssRules(shellCss)
      .filter(r => r.selector.includes('data-pane-resizing'))
      .flatMap(r => selectorList(r.selector))
      .filter(one => one.includes(WASH));
    expect(killed, 'the wash left the pane-resize kill list')
      .toEqual([`:root[data-pane-resizing] .${WASH}`]);
  });
});
