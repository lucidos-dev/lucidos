/**
 * A failed picker action keeps its message until the user acts again.
 *
 * The picker polls `fetchWorkspaces` every 2s. That function used to clear the
 * action error line on each success. So a failed Start, Save or Open showed its
 * reason for one tick at most, sometimes for no frame at all.
 *
 * A source scan, like the row-layout suite: the picker owns a control client
 * and many signals, and this suite has no DOM to mount it in.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source: string = readFileSync(resolve(here, '../WorkspacePicker.tsx'), 'utf-8');

/** The body of `async function <name>`, up to its closing brace at 2 spaces. */
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
