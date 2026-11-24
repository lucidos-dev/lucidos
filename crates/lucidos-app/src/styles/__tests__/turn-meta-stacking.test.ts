import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, rulesTargeting, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string): string => readFileSync(resolve(here, rel), 'utf8');

const RESPONSE_CSS = read('../chat/response.css');
const INITIATOR_CSS = read('../chat/input-messages.css');

const CLUSTERS = [
  { cls: 'response-meta', header: 'response-header', css: RESPONSE_CSS },
  { cls: 'initiator-meta', header: 'initiator-header', css: INITIATOR_CSS },
];

const CONTRACT: [string, string][] = [
  ['flex', '1 1 0'],
  ['flex-wrap', 'wrap'],
  ['justify-content', 'flex-end'],
  ['white-space', 'nowrap'],
  ['row-gap', '0'],
  ['column-gap', '0.5rem'],
];

const rulesFor = (cls: string, css: string): CssRule[] => rulesTargeting(css, cls);

function effective(rules: CssRule[], prop: string): string | undefined {
  let value: string | undefined;
  for (const rule of rules) {
    const own = rule.props.get(prop);
    if (own !== undefined) value = own;
  }
  return value;
}

function firstFieldRule(cls: string, css: string): CssRule | undefined {
  return cssRules(css).find(rule => rule.selector === `.${cls} > :first-child`);
}

describe('turn header meta stacking', () => {
  for (const { header, css } of CLUSTERS) {
    it(`.${header} aligns at flex-start on the shared row unit`, () => {
      const rules = rulesFor(header, css);
      expect(effective(rules, 'align-items'), `.${header} must align its fields at the top`)
        .toBe('flex-start');
      expect(effective(rules, 'min-height'), `.${header} must floor at the row unit`)
        .toBe('var(--turn-header-line)');
    });
  }

  it('sizes the actor/executor chip to the row unit, padding added back', () => {
    const chip = cssRules(INITIATOR_CSS).find(r => r.selector === '.initiator-actor, .response-executor');
    expect(chip, 'the shared actor/executor chip rule must exist').toBeDefined();
    expect(chip?.props.get('--actor-chip-pad-y')).toBe('0.125rem');
    expect(chip?.props.get('padding')).toBe('var(--actor-chip-pad-y) 0.375rem');
    expect(chip?.props.get('margin')).toBe('calc(-1 * var(--actor-chip-pad-y)) -0.375rem');
    expect(chip?.props.get('min-height'))
      .toBe('calc(var(--turn-header-line) + 2 * var(--actor-chip-pad-y))');
  });

  it('sizes the turn controls to the row unit', () => {
    expect(effective(rulesTargeting(INITIATOR_CSS, 'turn-controls'), 'min-height'))
      .toBe('var(--turn-header-line)');
  });

  for (const { cls, css } of CLUSTERS) {
    it(`sizes .${cls}'s first field to the row unit and centres its text`, () => {
      const rule = firstFieldRule(cls, css);
      expect(rule, `.${cls} > :first-child must be sized`).toBeDefined();
      expect(rule?.props.get('min-height')).toBe('var(--turn-header-line)');
      expect(rule?.props.get('display')).toBe('flex');
      expect(rule?.props.get('align-items')).toBe('center');
    });
  }

  for (const { cls, css } of CLUSTERS) {
    describe(`.${cls}`, () => {
      const rules = rulesFor(cls, css);

      it('is styled by at least one rule, so the scan is looking at something', () => {
        expect(rules.length, `no rule targets .${cls}`).toBeGreaterThan(0);
      });

      for (const [prop, want] of CONTRACT) {
        it(`sets ${prop}: ${want}`, () => {
          expect(effective(rules, prop), `.${cls} must end up at ${prop}: ${want}`).toBe(want);
        });
      }

      it('never goes rigid again, whatever a later rule says', () => {
        expect(effective(rules, 'flex-shrink'), `.${cls} must stay shrinkable`).toBeUndefined();
        expect(effective(rules, 'flex-basis'), `.${cls} must keep the zero basis from its shorthand`)
          .toBeUndefined();
      });

      it('does not lean on the auto margin the elastic basis made inert', () => {
        expect(effective(rules, 'margin-left'), `.${cls} right-aligns via justify-content now`)
          .not.toBe('auto');
      });
    });
  }

  it('keeps a timestamp after a status out of the cluster minimum', () => {
    for (const ts of ['response-timestamp', 'initiator-timestamp']) {
      const rule = cssRules(INITIATOR_CSS)
        .find(r => r.selector.split(',').some(s => s.trim() === `.exchange-status-label + .${ts}`));
      expect(rule, `.exchange-status-label + .${ts} must be sized`).toBeDefined();
      expect(rule?.props.get('width')).toBe('0');
      expect(rule?.props.get('flex')).toBe('0 0 max-content');
    }
  });

  it('defines the row unit the fields are sized to', () => {
    const base = read('../global/base.css');
    expect(base, 'base.css :root must define --turn-header-line')
      .toMatch(/--turn-header-line:\s*[^;]+;/);
  });

  it('gives the two headers the same contract, so a turn degrades as one', () => {
    const [response, initiator] = CLUSTERS.map(({ cls, css }) => {
      const rules = rulesFor(cls, css);
      const field = firstFieldRule(cls, css);
      return [
        ...CONTRACT.map(([prop]) => `${prop}: ${effective(rules, prop)}`),
        `first-field: ${field?.props.get('display')} ${field?.props.get('align-items')} ${field?.props.get('min-height')}`,
      ].join('; ');
    });
    expect(initiator, '.initiator-meta and .response-meta must wrap identically').toBe(response);
  });
});
