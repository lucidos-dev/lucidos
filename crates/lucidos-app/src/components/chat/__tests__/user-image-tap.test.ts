import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf-8');

/**
 * Regression: user-attached images live inside InitiatorPanel, not ResponsePanel.
 * The handleLinkClick delegation only fires on .response-content, so it can't
 * open the popup for .user-image-thumb. Each user image must own its onClick.
 *
 * Symptom that drove the fix: tapping an attached-image preview on iOS PWA did
 * nothing (broken on every platform, mobile just made it obvious).
 */
describe('user-image-thumb tap opens popup', () => {
  it('the <img class="user-image-thumb"> element has its own onClick attached', () => {
    const match = source.match(/<img[\s\S]*?class="user-image-thumb"[\s\S]*?\/>/);
    expect(match, 'user-image-thumb img element not found in ChatExchange.tsx').not.toBeNull();
    expect(match![0]).toMatch(/onClick=/);
  });

  it('handleLinkClick no longer references .user-image-thumb (dead delegation)', () => {
    // The handler is on .response-content; user images are in InitiatorPanel.
    // Listing .user-image-thumb in the closest() selector is dead code.
    const handlerMatch = source.match(/function handleLinkClick[\s\S]*?\n\s*\}/);
    expect(handlerMatch).not.toBeNull();
    expect(handlerMatch![0]).not.toContain('user-image-thumb');
  });

  // The collector matches via `el.src` (absolute). `blobPreviewUrl` returns a
  // relative path, so passing the closure value misses every match.
  it('user-image-thumb passes e.currentTarget.src (absolute) to the collector, not the closure src (relative)', () => {
    const match = source.match(/<img[\s\S]*?class="user-image-thumb"[\s\S]*?\/>/);
    expect(match, 'user-image-thumb img element not found').not.toBeNull();
    expect(match![0]).toMatch(/openImagePopupFromGroup\(\s*e\.currentTarget\.src/);
    expect(match![0]).not.toMatch(/openImagePopupFromGroup\(\s*src\b/);
  });
});
