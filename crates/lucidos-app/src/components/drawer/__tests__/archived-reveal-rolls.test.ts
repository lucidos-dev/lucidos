import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(resolve(here, '../ThreadDrawer.tsx'), 'utf-8');

describe('the archived-reveal toggle joins the drawer\'s disclosure keys', () => {
  it('passes revealedArchivedSet into useFlipTransitions alongside the other disclosure sets', () => {
    const call = source.match(/useFlipTransitions\([^;]*\);/);
    expect(call, 'no useFlipTransitions(...) call found in ThreadDrawer.tsx').not.toBeNull();
    expect(call![0]).toContain('revealedArchivedSet');
  });
});
