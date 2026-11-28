import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, relative, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { clientSourcePaths, styleSheetPaths } from './css-rule-helpers';

const here: string = dirname(fileURLToPath(import.meta.url));
const src: string = resolve(here, '../..');
const sdkSrc: string = resolve(src, '../../../packages/lucidos-sdk/src');

const RAW_ENV = /env\(\s*safe-area-inset-/;
const TOKEN_LINE = /^\s*--safe-area-(top|right|bottom|left):/;
const PROBE = 'utils/safeAreaFloor.ts';
const TOKEN_WITH_FALLBACK = /var\(--safe-area-(top|right|bottom|left),\s*env\(safe-area-inset-\1[^)]*\)\)/g;

describe('safe-area insets', () => {
  it('are read through the --safe-area-* tokens', () => {
    const raw: string[] = [];
    const sources = [...styleSheetPaths(src), ...clientSourcePaths(src), ...clientSourcePaths(sdkSrc)];
    for (const path of sources) {
      const file = relative(src, path);
      if (file === PROBE) continue;
      readFileSync(path, 'utf-8').split('\n').forEach((line: string, i: number) => {
        const comment = /^\s*(\/\/|\/?\*)/.test(line);
        const bare = line.replace(TOKEN_WITH_FALLBACK, '');
        if (RAW_ENV.test(bare) && !TOKEN_LINE.test(line) && !comment) raw.push(`${file}:${i + 1}`);
      });
    }
    expect(raw, 'read var(--safe-area-<side>) instead').toEqual([]);
  });

  it('let the floor raise each token', () => {
    const base = readFileSync(resolve(src, 'styles/global/base.css'), 'utf-8');
    for (const side of ['top', 'right', 'bottom', 'left']) {
      expect(base).toContain(
        `--safe-area-${side}: max(env(safe-area-inset-${side}, 0px), var(--safe-area-floor-${side}, 0px));`,
      );
    }
  });
});
