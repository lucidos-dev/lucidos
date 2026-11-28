import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { lucidosVersionLabel, lucidosVersionTooltip } from './lucidosVersion';

const here = dirname(fileURLToPath(import.meta.url));
const BRAND_MENU_ROWS = resolve(here, '../components/layout/BrandMenuRows.tsx');

describe('lucidosVersionLabel', () => {
  it('is the bare release when the code matches it', () => {
    expect(lucidosVersionLabel('0.24.1', false)).toBe('0.24.1');
  });

  it('marks a release the code has moved past with a trailing star', () => {
    expect(lucidosVersionLabel('0.24.1', true)).toBe('0.24.1 *');
  });

  it('names the destination while the release is unknown, rather than blanking', () => {
    expect(lucidosVersionLabel(null, false)).toBe('System');
    expect(lucidosVersionLabel(null, true)).toBe('System');
  });
});

describe('lucidosVersionTooltip', () => {
  it('spells the star out, because an asterisk alone says nothing', () => {
    expect(lucidosVersionTooltip('0.24.1', true)).toContain('0.24.1');
    expect(lucidosVersionTooltip('0.24.1', true)).toMatch(/changed since this release/);
  });

  it('is just the product and its version when clean', () => {
    expect(lucidosVersionTooltip('0.24.1', false)).toBe('Lucidos 0.24.1');
    expect(lucidosVersionTooltip('0.24.1', false)).not.toContain('*');
  });

  it('says nothing at all with no release to name', () => {
    expect(lucidosVersionTooltip(null, false)).toBeUndefined();
    expect(lucidosVersionTooltip(null, true)).toBeUndefined();
  });
});

describe('the Lucidos menu identity row', () => {
  const source = readFileSync(BRAND_MENU_ROWS, 'utf-8');

  it('reads the umbrella release and its dirty flag', () => {
    expect(source).toMatch(/lucidosVersionLabel\(release, releaseDirty\)/);
    expect(source).toMatch(/lucidosRelease\.value/);
    expect(source).toMatch(/lucidosReleaseDirty\.value/);
  });

  it('does not reach for the per-device client version', () => {
    expect(
      source.includes('clientVersionLabel'),
      'the identity row names Lucidos itself, so it must not show a per-platform client build id',
    ).toBe(false);
  });
});
