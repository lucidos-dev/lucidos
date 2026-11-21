/**
 * The slowness bar's height reservation. `--app-header-bottom` sums every
 * banner's property, so toasts and the drawer start below whichever are up.
 */
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { SLOWNESS_BANNER_HEIGHT_VAR } from '../SlownessBanner';

const BASE_CSS = readFileSync(
  resolve(dirname(fileURLToPath(import.meta.url)), '../../../styles/global/base.css'),
  'utf-8',
) as string;

describe('the slowness bar reserves its height below the header', () => {
  it('is a term of --app-header-bottom', () => {
    const anchor = BASE_CSS.split('\n').find((line: string) => line.includes('--app-header-bottom: calc('));
    expect(anchor).toContain(`var(${SLOWNESS_BANNER_HEIGHT_VAR}`);
  });
});
