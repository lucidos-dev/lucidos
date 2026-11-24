import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const connection = readFileSync(resolve(here, '../actions/connection.ts'), 'utf-8');

describe('resume sync', () => {
  it('re-reads the plugin catalog', () => {
    const body = connection.slice(connection.indexOf('function runResumeSync'));
    expect(body.slice(0, body.indexOf('\n}\n'))).toMatch(/resyncPluginCatalog\(\);/);
  });
});
