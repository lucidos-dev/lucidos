import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

import { cssRules } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../../styles/components.css'), 'utf-8');

/** Every control the tap hides. */
const CHROME = [
  'image-popup-close',
  'floating-mobile-close',
  'image-popup-nav',
  'image-popup-counter',
  'image-popup-zoom',
];

/** The rule listing all five, either at rest or under `.chrome-hidden`. A rule
 *  governs the change TOWARD its own state, so the resting one owns the arrival
 *  and the hidden one owns the departure. */
function chromeGroup(hidden: boolean) {
  const rule = cssRules(css).find(
    r => CHROME.every(cls => r.selector.includes(cls))
      && r.selector.includes('chrome-hidden') === hidden,
  );
  expect(rule, `no ${hidden ? 'hidden' : 'resting'} rule covering all five controls`).toBeDefined();
  return rule!;
}

describe('image popup — tap toggles chrome (close, nav, counter)', () => {
  it('uses a chromeHidden state', () => {
    expect(source).toMatch(/chromeHidden/);
  });

  it('the image-popup-content element gets the chrome-hidden class when chromeHidden is true', () => {
    expect(source).toMatch(/chrome-hidden/);
  });

  // chromeGroup itself is the "all five are covered" assertion: it fails when no
  // rule names every one of them. So this reads the declarations and nothing
  // else. The hit target dies at once rather than fading with the opacity.
  it('CSS hides close, mobile-close, nav, counter and zoom when chrome-hidden is set', () => {
    const hidden = chromeGroup(true).props;
    expect(hidden.get('opacity')).toBe('0');
    expect(hidden.get('pointer-events')).toBe('none');
  });

  it('fades both ways, on a duration the animation-speed slider scales', () => {
    for (const hidden of [false, true]) {
      const transition = chromeGroup(hidden).props.get('transition');
      expect(transition, `the ${hidden ? 'hidden' : 'resting'} rule sets no transition`).toBeDefined();
      expect(transition).toContain('opacity var(--duration-slow)');
    }
  });

  it('decelerates on the way in and accelerates on the way out', () => {
    // Both directions ran ease-out once. A departure on that curve is all but
    // gone in its first third, which is what read as no transition.
    expect(chromeGroup(false).props.get('transition')).toContain('var(--duration-slow) ease-out');
    expect(chromeGroup(true).props.get('transition')).toContain('var(--duration-slow) ease-in');
  });

  it('a click on the strip toggles chromeHidden (registered via addEventListener)', () => {
    expect(source).toMatch(/strip\.addEventListener\(\s*['"]click['"]/);
    expect(source).toMatch(/setChromeHidden\(\s*v\s*=>\s*!v\s*\)/);
  });

  it('chromeHidden resets to false when the popup opens', () => {
    expect(source).toMatch(/setChromeHidden\(\s*false\s*\)/);
  });

  it('does not toggle on double-click (zoom gesture wins)', () => {
    expect(source).toMatch(/e\.detail\s*>\s*1|detail\s*!==?\s*1/);
  });
});
