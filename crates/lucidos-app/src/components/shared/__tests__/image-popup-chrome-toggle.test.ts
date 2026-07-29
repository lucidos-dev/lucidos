import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../../styles/components.css'), 'utf-8');

describe('image popup — tap toggles chrome (close, nav, counter)', () => {
  it('uses a chromeHidden state', () => {
    expect(source).toMatch(/chromeHidden/);
  });

  it('the image-popup-content element gets the chrome-hidden class when chromeHidden is true', () => {
    expect(source).toMatch(/chrome-hidden/);
  });

  it('CSS hides close, mobile-close, nav, and counter when chrome-hidden is set', () => {
    const block = css.match(/\.image-popup-content\.chrome-hidden[\s\S]*?\}/);
    expect(block, 'no .image-popup-content.chrome-hidden CSS block found').not.toBeNull();
    const text = block![0];
    expect(text).toContain('image-popup-close');
    expect(text).toContain('floating-mobile-close');
    expect(text).toContain('image-popup-nav');
    expect(text).toContain('image-popup-counter');
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
