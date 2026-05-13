import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');
const storeSource = readFileSync(resolve(here, '../../../store/store.ts'), 'utf-8');

describe('image-preview-thumb tap opens popup with strip traversal', () => {
  const stripImgs = promptSource.match(/<img[\s\S]*?class="image-preview-thumb"[\s\S]*?\/>/g) ?? [];

  it('every <img class="image-preview-thumb"> in PromptInput has an onClick', () => {
    expect(stripImgs.length, 'no image-preview-thumb img elements found').toBeGreaterThan(0);
    for (const tag of stripImgs) {
      expect(tag, 'image-preview-thumb missing onClick').toMatch(/onClick=/);
    }
  });

  it('image-preview-thumb onClick routes through the group collector, not the single opener', () => {
    for (const tag of stripImgs) {
      expect(tag).toMatch(/openImagePopupFromGroup\(/);
      expect(tag).not.toMatch(/onClick=\{\s*\(\)\s*=>\s*openImagePopup\(/);
    }
  });

  it('image-preview-thumb passes e.currentTarget so the collector can scope to the strip', () => {
    for (const tag of stripImgs) {
      expect(tag).toMatch(/openImagePopupFromGroup\([^)]*e\.currentTarget/);
    }
  });

  it('openImagePopupFromGroup walks up to the prompt strip as well as the thread', () => {
    const fnMatch = storeSource.match(/export function openImagePopupFromGroup[\s\S]*?\n\}/);
    expect(fnMatch, 'openImagePopupFromGroup not exported from store.ts').not.toBeNull();
    const body = fnMatch![0];
    expect(body, 'closest() must include the prompt strip container').toMatch(/\.image-preview-strip/);
    expect(body, 'querySelectorAll must include the strip thumb class').toMatch(/\.image-preview-thumb/);
    expect(body, 'thread-content container still supported').toMatch(/\.thread-content/);
  });
});
