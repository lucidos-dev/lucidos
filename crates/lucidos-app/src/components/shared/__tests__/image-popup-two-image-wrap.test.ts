import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ImagePopup.tsx'), 'utf-8');

describe('image popup — two-image wrap (no black flash)', () => {
  it('renders an extra mirror slide for n=2 to fill the side shortestDelta cannot reach', () => {
    expect(source).toMatch(/total\s*===\s*2/);
    expect(source).toMatch(/left:\s*-100%/);
  });

  it('the mirror shows the OTHER image (1 - state.index), not the current one', () => {
    expect(source).toMatch(/state\.images\[\s*1\s*-\s*state\.index\s*\]/);
  });
});
