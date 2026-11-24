import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { cssRules, type CssRule } from './css-rule-helpers';

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, '../chat/input-messages.css'), 'utf8');
const icons = readFileSync(resolve(here, '../../components/shared/icons.tsx'), 'utf8');

function rule(selector: string): CssRule {
  const found = cssRules(css).filter(r => r.selector === selector);
  expect(found, `expected exactly one \`${selector}\` rule`).toHaveLength(1);
  return found[0];
}

const MEASURED = {
  'claude-icon': 0.997,
};

describe('the composer agent mark is sized by ink, not by box', () => {
  it('targets the run\'s own ink, derived from the run\'s glyph token', () => {
    expect(rule('.commands-btn').props.get('--run-ink'))
      .toBe('calc(var(--icon-size-lg) * 0.75)');
  });

  it('defaults an unmeasured glyph to the run\'s box, changing nothing', () => {
    expect(rule('.commands-btn').props.get('--mark-art-fill')).toBe('0.75');
  });

  for (const [cls, fill] of Object.entries(MEASURED)) {
    it(`divides .${cls} by its measured ${fill} fill`, () => {
      expect(rule(`.commands-btn .${cls}`).props.get('--mark-art-fill')).toBe(String(fill));
    });
  }

  it('sizes both axes by the formula, and beats the run\'s own rule outright', () => {
    const sizing = rule('.icon-btn.header-icon.commands-btn svg');
    for (const axis of ['width', 'height']) {
      expect(sizing.props.get(axis)).toBe('calc(var(--run-ink) / var(--mark-art-fill))');
    }
  });

  it('never hands a mark a literal size, which is how a measurement goes stale', () => {
    for (const cls of Object.keys(MEASURED)) {
      const props = rule(`.commands-btn .${cls}`).props;
      expect(props.get('width'), `.${cls} must be sized by the formula`).toBeUndefined();
      expect(props.get('height'), `.${cls} must be sized by the formula`).toBeUndefined();
    }
  });
});

describe('the fill fractions still describe the artwork they were measured from', () => {
  it('keeps the class each rule selects on, or the rule silently stops applying', () => {
    for (const cls of Object.keys(MEASURED)) {
      expect(icons, `icons.tsx no longer renders class="${cls}"`).toContain(`class="${cls}"`);
    }
  });
});
