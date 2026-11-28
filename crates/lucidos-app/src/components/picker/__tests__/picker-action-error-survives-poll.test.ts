import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../WorkspacePicker.tsx'), 'utf-8');

function functionBody(name: string): string {
  const start = source.indexOf(`async function ${name}(`);
  expect(start).toBeGreaterThan(-1);
  const end = source.indexOf('\n  }\n', start);
  expect(end).toBeGreaterThan(start);
  return source.slice(start, end);
}

describe('the picker error line outlives the background poll', () => {
  it('the poll goes through fetchWorkspaces', () => {
    expect(functionBody('pollRefresh')).toContain('await fetchWorkspaces()');
  });

  it('fetchWorkspaces never writes the error line', () => {
    expect(functionBody('fetchWorkspaces')).not.toMatch(/error\.value\s*=/);
  });

  it('a new action is what clears it', () => {
    expect(functionBody('withBusy')).toMatch(/error\.value\s*=\s*null/);
  });
});
