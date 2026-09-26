/**
 * Disk Usage recovers from a failed read.
 *
 * Its loadables are module-level signals, and the page used to read only from
 * `not-loaded`. One failed read then showed the error on every later visit,
 * with no Retry, until the whole app reloaded.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { diskUsageLoadOwed } from './DiskUsagePage';

describe('diskUsageLoadOwed', () => {
  it('reads on a first visit', () => {
    expect(diskUsageLoadOwed({ status: 'not-loaded' })).toBe(true);
  });

  it('reads again after a failure', () => {
    expect(diskUsageLoadOwed({ status: 'failed', error: 'engine restarting' })).toBe(true);
  });

  it('leaves a read in flight alone', () => {
    expect(diskUsageLoadOwed({ status: 'loading' })).toBe(false);
  });

  it('keeps a loaded page as it is', () => {
    expect(diskUsageLoadOwed({ status: 'loaded', data: [] })).toBe(false);
  });
});

describe('the failed page', () => {
  it('offers a Retry that reads the inventory again', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source: string = readFileSync(resolve(here, 'DiskUsagePage.tsx'), 'utf-8');
    expect(source).toContain('onRetry={() => void loadInventory()}');
  });
});
