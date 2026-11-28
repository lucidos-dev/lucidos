import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readdirSync, readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import type { StepOutcome } from '../../store/types';

const here: string = dirname(fileURLToPath(import.meta.url));
const steps: string = readFileSync(resolve(here, '../steps.css'), 'utf8');

const OUTCOMES: Record<StepOutcome, true> = {
  pending: true,
  success: true,
  error: true,
  unfinished: true,
  blocked: true,
  denied: true,
};
const ALL = Object.keys(OUTCOMES) as StepOutcome[];
const MARKED = ALL.filter(o => o !== 'pending');

describe('step outcome styling', () => {
  it.each(MARKED)('%s draws a mark on the transcript row', (outcome) => {
    expect(steps).toContain(`.inline-step.${outcome} .step-icon`);
  });

  it.each(ALL)('%s tints the step detail head icon', (outcome) => {
    expect(steps).toContain(`.step-detail-icon.${outcome}`);
  });

  it('the running row is deliberately unmarked', () => {
    expect(steps).not.toContain('.inline-step.pending .step-icon {');
  });

  it.each(ALL)('no stylesheet styles a bare .%s class', (outcome) => {
    const bare = new RegExp(`(?:^|[{},])\\s*\\.${outcome}\\s*(?=[,{:])`, 'm');
    const hits = cssFiles(resolve(here, '..')).filter(f => bare.test(readFileSync(f, 'utf8')));
    expect(hits).toEqual([]);
  });
});

function cssFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry: { name: string; isDirectory(): boolean }) => {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) return cssFiles(path);
    return entry.name.endsWith('.css') ? [path] : [];
  });
}
