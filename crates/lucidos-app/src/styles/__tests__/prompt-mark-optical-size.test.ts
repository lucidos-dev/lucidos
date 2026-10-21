/**
 * The composer's agent mark reads at the size its neighbours do.
 *
 * The prompt bar's icon run is `.icon-btn.header-icon`: a 2.25rem box holding a
 * `--icon-size-lg` glyph. Giving the leading agent control that class was not
 * enough, because the same BOX is not the same SIZE. Todo, the subscription
 * clock and attach are drawn to one grid and each paints exactly 0.75 of its
 * own viewBox; the three agent marks are foreign artwork and paint 0.997
 * (Claude) and 0.88 (Codex). At one box, Claude reads a third larger than the
 * checkbox beside it. The user reported exactly that.
 *
 * So each mark is sized by its own fill fraction to land the run's ink. This
 * pins the two halves together: the formula that does the dividing, and the
 * fractions it divides by. A fraction is a MEASUREMENT of the artwork, so it is
 * only correct while the artwork is unchanged.
 */
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

/** The one rule with this exact selector. Parsed rather than brace-matched on
 *  the first textual hit, because these selectors are descendant ones, which
 *  `rulesTargeting` deliberately does not answer, and because a mark can carry
 *  more than one rule. */
function rule(selector: string): CssRule {
  const found = cssRules(css).filter(r => r.selector === selector);
  expect(found, `expected exactly one \`${selector}\` rule`).toHaveLength(1);
  return found[0];
}

/** Painted extent as a fraction of the glyph's own viewBox, the longer axis,
 *  stroke included, via `getBBox({ stroke: true })`. The run's three stroke
 *  icons all come out at exactly 0.750. Measured against the built bundle in
 *  both engines the app ships on, Chromium and WebKit, which agree to the
 *  hundredth of a px on every glyph here. */
const MEASURED = {
  'claude-icon': 0.997,
  'codex-icon': 0.88,
};

describe('the composer agent mark is sized by ink, not by box', () => {
  it('targets the run\'s own ink, derived from the run\'s glyph token', () => {
    // 0.75 is the fraction the row's other three glyphs paint, so --run-ink is
    // literally "the ink a neighbour lands". Derived from --icon-size-lg rather
    // than a rem literal, so retuning the run retunes the marks with it.
    expect(rule('.commands-btn').props.get('--run-ink'))
      .toBe('calc(var(--icon-size-lg) * 0.75)');
  });

  it('defaults an unmeasured glyph to the run\'s box, changing nothing', () => {
    // A mark nobody has measured divides by the run's own fraction and comes
    // out at --icon-size-lg exactly, which is where it would have been anyway.
    expect(rule('.commands-btn').props.get('--mark-art-fill')).toBe('0.75');
  });

  for (const [cls, fill] of Object.entries(MEASURED)) {
    it(`divides .${cls} by its measured ${fill} fill`, () => {
      expect(rule(`.commands-btn .${cls}`).props.get('--mark-art-fill')).toBe(String(fill));
    });
  }

  it('sizes both axes by the formula, and beats the run\'s own rule outright', () => {
    // `.icon-btn.header-icon svg` (host-components.css) is two classes plus the
    // type. Three classes here, so this wins on specificity rather than on the
    // order the two sheets happen to be imported in.
    const sizing = rule('.icon-btn.header-icon.commands-btn svg');
    for (const axis of ['width', 'height']) {
      expect(sizing.props.get(axis)).toBe('calc(var(--run-ink) / var(--mark-art-fill))');
    }
  });

  it('never hands a mark a literal size, which is how a measurement goes stale', () => {
    // The whole point is that the number lives in --mark-art-fill next to the
    // comment saying it was measured. A rem literal on one of these rules is
    // the same magic number with nothing pointing at the artwork.
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
