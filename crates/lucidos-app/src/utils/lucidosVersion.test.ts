import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { lucidosVersionLabel, lucidosVersionTooltip } from './lucidosVersion';

const here = dirname(fileURLToPath(import.meta.url));
const HEADER_MARK = resolve(here, '../components/layout/HeaderMark.tsx');

describe('lucidosVersionLabel', () => {
  it('is the bare release when the code matches it', () => {
    expect(lucidosVersionLabel('0.24.1', false)).toBe('0.24.1');
  });

  it('marks a release the code has moved past with a trailing star', () => {
    expect(lucidosVersionLabel('0.24.1', true)).toBe('0.24.1 *');
  });

  it('names the destination while the release is unknown, rather than blanking', () => {
    // `lucidosRelease` is null until /health answers, and on any engine that
    // answers without one. The row must keep its shape either way.
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
    // An `aria-label` replaces the visible text as the accessible name, so an
    // invented sentence here would make the row unaddressable by the word the
    // user can see on it.
    expect(lucidosVersionTooltip(null, false)).toBeUndefined();
    expect(lucidosVersionTooltip(null, true)).toBeUndefined();
  });
});

describe('the Lucidos menu identity row', () => {
  /**
   * The row shows the PRODUCT's version, not this device's copy of it.
   *
   * It has already been the other thing twice: the engine's CalVer (which is the
   * workspace binary, not Lucidos), then `clientVersionLabel()` (which answers
   * "dev" on a Vite dev server, a hex build id on a built web client, and the
   * shell's app version inside Tauri, so one install names itself three ways).
   * Both readings are honest about something, and neither answers "what am I
   * running?". A source-scan, matching the `clientVersionSource` precedent: the
   * assertion is about which SOURCE the row reads, which a render test could
   * only observe indirectly.
   */
  const source = readFileSync(HEADER_MARK, 'utf-8');

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
