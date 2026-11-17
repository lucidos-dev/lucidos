import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const spec = readFileSync(resolve(here, '../../../../e2e/preview-image-popup-mobile.spec.ts'), 'utf-8');
const popup = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');

describe('image popup mobile spec: when the fixture counts as loaded', () => {
  it('the popup writes its transform at mount, before any image has loaded', () => {
    expect(popup).toMatch(/function zoomToFit\(\) \{\s*zoomRef\.current = \{ scale: fitScale\(\)/);
    expect(popup).toMatch(/if \(!layout\) return 1;/);
  });

  it('so the spec waits for a decoded width, not the transform alone', () => {
    const wait = spec.match(/waitForFunction\(\(\) => \{[^}]*image-popup-slide img[^}]*\}/);
    expect(wait, 'no readiness wait found').not.toBeNull();
    expect(wait![0]).toMatch(/naturalWidth\s*>\s*0/);
  });
});
