import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';
// @ts-expect-error same
import { dirname, resolve, relative } from 'node:path';

/**
 * Every reasoning picker, on BOTH surfaces, must ask what the engine will
 * actually send. `store/modelSelection.ts` is the single module that answers,
 * and `useModelSelection` is the single hook that resolves, clamps and reports
 * a *model selection*.
 *
 * The two surfaces used to answer separately, and only one of them clamped. The
 * Lucidos Agent filtered `REASONING_LEVELS` against the registry's
 * `reasoning_efforts`; the coding-agent menu filtered its option list against
 * each effort's `supported_models`. Calling a raw helper directly reintroduces
 * the drift that broke a local-model turn: the picker offered `max`, the wire
 * sent something the local server rejected, and both layers were "correct" by
 * their own rule. See
 * `docs/plans/2026-08-12-reasoning-effort-follows-model.md`.
 *
 * A source scan rather than a render test, matching `skeleton-guard.test.ts`:
 * the invariant is which function a call site reaches for, and the failure mode
 * is a NEW surface added later, which no existing render test would cover.
 */

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..'); // crates/lucidos-app/src

/** The one module allowed to call the raw, heuristic-capable pair. */
const TIERS_MODULE = resolve(SRC, 'store/modelSelection.ts');

/** The one component that renders a *model selection*, on every surface. */
const PICKER = resolve(SRC, 'components/shared/ModelSelectionPicker.tsx');

/** Building either step's rows. Doing it outside the picker IS a second
 *  picker, whatever it renders them with. */
const STEP_BUILDERS = /\b(modelStepOptions|tierStepOptions|modelStepCommit)\b/;

/** The raw, heuristic-capable functions, imported from `store/models`. */
const RAW_IMPORT =
  /import\s*\{[^}]*\b(availableReasoningLevels|clampReasoningEffort)\b[^}]*\}\s*from\s*['"][^'"]*\/models['"]/;

/** The retired coding-agent wire field. The engine transposes the matrix onto
 *  the model rows now. A surface reading this went back to deriving its own
 *  answer, from a field that is no longer served. */
const RETIRED_WIRE_FIELD = /\bsupported_models\b/;

function sourceFiles(dir: string): string[] {
  const out: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = resolve(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name === '__tests__') continue;
      out.push(...sourceFiles(full));
    } else if (/\.tsx?$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
      out.push(full);
    }
  }
  return out;
}

describe('reasoning pickers go through the model-to-tiers module', () => {
  it('only the tiers module imports the raw heuristic-capable pair', () => {
    const offenders = sourceFiles(SRC)
      .filter((f) => f !== TIERS_MODULE)
      .filter((f) => RAW_IMPORT.test(readFileSync(f, 'utf8')))
      .map((f) => relative(SRC, f));

    expect(
      offenders,
      'Resolve tiers through `store/modelSelection` (and pick through '
        + '`useModelSelection`), so the picker offers what the engine will send.',
    ).toEqual([]);
  });

  it('nothing reads the retired coding-agent compatibility field', () => {
    // The tiers module names it in prose, explaining what it replaced.
    const offenders = sourceFiles(SRC)
      .filter((f) => f !== TIERS_MODULE)
      .filter((f) => RETIRED_WIRE_FIELD.test(readFileSync(f, 'utf8')))
      .map((f) => relative(SRC, f));

    expect(
      offenders,
      'The engine serves `reasoning_efforts` per model row now. Read that, '
        + 'via `tiersOf`, rather than an effort-to-models list.',
    ).toEqual([]);
  });

  it('the tiers module still answers for both surfaces', () => {
    // Guards the guard: renamed away, the scans above would pass vacuously
    // while every surface went back to deriving its own answer.
    const src = readFileSync(TIERS_MODULE, 'utf8');
    expect(src).toMatch(/export function lucidosTiers\b/);
    expect(src).toMatch(/export function tiersOf\b/);
    expect(src).toMatch(/export function clampToOffered\b/);
  });

  it('the hook is the one place a pick clamps', () => {
    const hook = readFileSync(resolve(SRC, 'hooks/useModelSelection.ts'), 'utf8');
    expect(hook).toMatch(/export function useModelSelection\b/);
    expect(hook).toMatch(/clampToOffered/);
  });

  it('a model selection is ONE pick, however many steps reach it', () => {
    // A separate Reasoning control needs its own option list and its own pick.
    // The hook offers neither, and the two scans above stop a surface deriving
    // them. So the pair cannot be split back into two controls.
    const hook = readFileSync(resolve(SRC, 'hooks/useModelSelection.ts'), 'utf8');
    expect(hook).toMatch(/\bpick: \(encoded: string\)/);
    expect(hook).not.toMatch(/\bpickEffort\b|\bpickModel\b|\beffortOptions\b|\bmodelOptions\b/);
  });

  it('one picker builds the steps, so no surface can grow its own', () => {
    // Four surfaces mount it: both prompt-bar control menus, the Settings field
    // and the trigger form. A second copy of the step logic is how a picker's
    // behaviour starts depending on which file it was written in.
    const offenders = sourceFiles(SRC)
      .filter((f) => f !== PICKER)
      .filter((f) => STEP_BUILDERS.test(readFileSync(f, 'utf8')))
      .map((f) => relative(SRC, f));

    expect(
      offenders,
      'Mount `ModelSelectionPicker`; it owns both steps, the filter and the '
        + 'keyboard. Only it builds the step rows.',
    ).toEqual([]);
  });

  it('the picker still owns both steps', () => {
    // Guards the guard: renamed away, the scan above would pass vacuously.
    const src = readFileSync(PICKER, 'utf8');
    expect(src).toMatch(/export function modelStepOptions\b/);
    expect(src).toMatch(/export function tierStepOptions\b/);
    expect(src).toMatch(/export function ModelSelectionPicker\b/);
  });
});
