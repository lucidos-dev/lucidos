import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf8');

describe('ThreadView republishes the scrollbar gutter', () => {
  it('runs again when the thread lands in the map, not on mount alone', () => {
    const effect = SOURCE.match(/useLayoutEffect\(\(\) => \{ publishScrollbarGutter\(\); \}, (\[[^\]]*\])\)/);
    expect(effect).not.toBeNull();
    expect(effect![1]).not.toBe('[]');
    expect(effect![1]).toContain('eventThread');
  });
});
