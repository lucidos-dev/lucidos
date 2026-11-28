import { describe, it, expect } from 'vitest';
// @ts-expect-error — Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error — same
import { dirname, resolve } from 'node:path';
// @ts-expect-error — same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../chat-exchange-parts.tsx'), 'utf-8');
const routerSource = readFileSync(resolve(here, '../../shared/markdownLinkClick.ts'), 'utf-8');

describe('user-image-thumb tap opens popup', () => {
  it('the <BlobImage class="user-image-thumb"> element has its own onClick attached', () => {
    const match = source.match(/<BlobImage[\s\S]*?class="user-image-thumb"[\s\S]*?\/>/);
    expect(match, 'user-image-thumb BlobImage element not found in chat-exchange-parts.tsx').not.toBeNull();
    expect(match![0]).toMatch(/onClick=/);
  });

  it('uses BlobImage (self-healing) for the inline user-image-thumb, not a bare <img>', () => {
    expect(source).toMatch(/<BlobImage[\s\S]*?class="user-image-thumb"/);
    expect(source).not.toMatch(/<img[\s\S]*?class="user-image-thumb"/);
  });

  it('the markdown link router no longer references .user-image-thumb (dead delegation)', () => {
    const handlerMatch = routerSource.match(/function handleMarkdownLinkClick[\s\S]*?\n\}\n/);
    expect(handlerMatch).not.toBeNull();
    expect(handlerMatch![0]).not.toContain('user-image-thumb');
  });

  it('user-image-thumb passes e.currentTarget.src (absolute) to the collector, not the closure src (relative)', () => {
    const match = source.match(/<BlobImage[\s\S]*?class="user-image-thumb"[\s\S]*?\/>/);
    expect(match, 'user-image-thumb BlobImage element not found').not.toBeNull();
    expect(match![0]).toMatch(/openImagePopupFromGroup\(\s*e\.currentTarget\.src/);
    expect(match![0]).not.toMatch(/openImagePopupFromGroup\(\s*src\b/);
  });
});
