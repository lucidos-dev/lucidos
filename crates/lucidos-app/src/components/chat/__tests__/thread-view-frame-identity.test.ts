import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf8');

function openingTags(name: string): string[] {
  const pattern = new RegExp(`<${name}(?=[\\s/>])[^>]*>`, 'g');
  return SOURCE.match(pattern) ?? [];
}

describe("ThreadView's two trees keep one skeleton overlay", () => {
  it('renders the overlay in exactly the two returns this is about', () => {
    expect(openingTags('ThreadSkeletonOverlay')).toHaveLength(2);
  });

  it('keys every overlay, so the diff matches it across the switch', () => {
    for (const tag of openingTags('ThreadSkeletonOverlay')) {
      expect(tag, tag).toContain('key="skeleton"');
    }
  });

  it('keys the wrap the overlay lives in, which the header displaces', () => {
    const wraps = openingTags('div').filter((t) => t.includes('class="thread-content-wrap'));
    expect(wraps).toHaveLength(2);
    for (const tag of wraps) {
      expect(tag, tag).toContain('key="wrap"');
    }
  });

  it('keys the scroll container beside it, so neither is matched by index', () => {
    const contents = openingTags('div').filter((t) => t.includes('class="thread-content visible"'));
    expect(contents).toHaveLength(2);
    for (const tag of contents) {
      expect(tag, tag).toContain('key="content"');
    }
  });
});
