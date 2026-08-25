/**
 * Every step outcome is drawn, in both places a step is drawn.
 *
 * `StepOutcome` doubles as the CSS class name on the transcript row
 * (`.inline-step.<outcome>`) and on the detail modal's status word
 * (`.step-detail-status.<outcome>`). `stepStatus` returns `className:
 * StepOutcome`, so `tsc` catches an outcome the LABEL forgot. Nothing else in
 * the gate catches an outcome the STYLESHEET forgot: `tsc` does not read CSS,
 * and `vite build` fails only on a syntax error. The row then draws its mark in
 * whatever colour the text around it happens to be, which is a passing build
 * and a wrong screen.
 *
 * `'pending'` is the deliberate exception on the row half. The running row
 * carries no mark, its shimmering description being the affordance, and
 * `steps.css` says so where the rule would be.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import type { StepOutcome } from '../../store/types';

const here: string = dirname(fileURLToPath(import.meta.url));
const steps: string = readFileSync(resolve(here, '../steps.css'), 'utf8');

/** Every member of the union. The `Record` is what makes the scan exhaustive:
 *  a new `StepOutcome` fails `tsc` here until it is listed, and therefore until
 *  somebody has decided how it looks. */
const OUTCOMES: Record<StepOutcome, true> = {
  pending: true,
  success: true,
  error: true,
  unfinished: true,
  blocked: true,
  denied: true,
};
const ALL = Object.keys(OUTCOMES) as StepOutcome[];
/** The row half. `'pending'` is exempt: see the header. */
const MARKED = ALL.filter(o => o !== 'pending');

describe('step outcome styling', () => {
  it.each(MARKED)('%s draws a mark on the transcript row', (outcome) => {
    expect(steps).toContain(`.inline-step.${outcome} .step-icon`);
  });

  it.each(ALL)('%s tints the step detail status word', (outcome) => {
    expect(steps).toContain(`.step-detail-status.${outcome}`);
  });

  it('the running row is deliberately unmarked, and says so', () => {
    expect(steps).not.toContain('.inline-step.pending .step-icon {');
    expect(steps).toContain('There is deliberately no `.inline-step.pending .step-icon` rule');
  });
});
