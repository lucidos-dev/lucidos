import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { block, decl } from '../../../styles/__tests__/css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const styles = (rel: string): string =>
  readFileSync(resolve(here, '../../../styles', rel), 'utf-8');

const baseCss = styles('global/base.css');
const shellCss = styles('panels/shell.css');
const componentsCss = styles('components.css');
const mobileCss = styles('mobile.css');

describe('below-header anchor (--app-header-bottom)', () => {
  const rootTokens = block(baseCss, ':root');

  it('base.css defines the header height and the header-bottom anchor', () => {
    expect(decl(rootTokens, '--app-header-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')).not.toBeNull();
  });

  it('the anchor includes the reclaimed macOS title-bar band', () => {
    const anchor = decl(rootTokens, '--app-header-bottom')!;
    expect(anchor).toContain('var(--titlebar-inset');
    expect(anchor).toContain('var(--app-header-height');
  });

  it('the anchor carries the app-shell banner height', () => {
    expect(decl(rootTokens, '--app-banner-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')!).toContain('var(--app-banner-height');
  });

  it('the anchor carries the connection banner as a second, distinct term', () => {
    expect(decl(rootTokens, '--app-conn-banner-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')!).toContain('var(--app-conn-banner-height');
  });

  it('the anchor carries the ingress banner as a third, distinct term', () => {
    expect(decl(rootTokens, '--app-ingress-banner-height')).not.toBeNull();
    expect(decl(rootTokens, '--app-header-bottom')!).toContain('var(--app-ingress-banner-height');
  });

  it('.app-header sizes itself from the same height token', () => {
    expect(decl(block(shellCss, '.app-header {'), 'height')).toBe('var(--app-header-height)');
  });

  it('the toast stack starts at the header bottom, not a copy of its height', () => {
    const top = decl(block(componentsCss, '.toast-container {'), 'top')!;
    expect(top).toContain('var(--app-header-bottom)');
    expect(top).not.toMatch(/2\.75rem|2\.5rem/);
  });

  it('the desktop drawer backdrop shares the anchor instead of restating it', () => {
    expect(decl(block(mobileCss, '.drawer-backdrop {'), 'top')).toBe('var(--app-header-bottom)');
    const desktop = block(mobileCss, '@media (--desktop-layout)');
    expect(decl(block(desktop, '.drawer-backdrop {'), 'top')).toBeNull();
  });

  it('the mobile layout redefines the anchor from its measured header height', () => {
    const mobileRoot = decl(
      block(block(mobileCss, '@media (--phone-layout)'), ':root'),
      '--app-header-bottom',
    );
    expect(mobileRoot).toContain('var(--mobile-header-height');
  });

  it('no stale copy of the header geometry survives', () => {
    for (const [name, css] of [
      ['components.css', componentsCss],
      ['mobile.css', mobileCss],
    ] as const) {
      expect(css, `${name} restates the header height`).not.toMatch(/2\.5rem \+ 0\.25rem/);
    }
  });
});
