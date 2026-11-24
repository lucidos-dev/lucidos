import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';
// @ts-expect-error same
import { dirname, resolve, relative } from 'node:path';


const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');

const TIERS_MODULE = resolve(SRC, 'store/modelSelection.ts');

const PICKER = resolve(SRC, 'components/shared/ModelSelectionPicker.tsx');

const STEP_BUILDERS = /\b(modelStepOptions|tierStepOptions|modelStepCommit)\b/;

const RAW_IMPORT =
  /import\s*\{[^}]*\b(availableReasoningLevels|clampReasoningEffort)\b[^}]*\}\s*from\s*['"][^'"]*\/models['"]/;

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
    const hook = readFileSync(resolve(SRC, 'hooks/useModelSelection.ts'), 'utf8');
    expect(hook).toMatch(/\bpick: \(encoded: string, provider\?: string\)/);
    expect(hook).not.toMatch(/\bpickEffort\b|\bpickModel\b|\beffortOptions\b|\bmodelOptions\b/);
  });

  it('one picker builds the steps, so no surface can grow its own', () => {
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
    const src = readFileSync(PICKER, 'utf8');
    expect(src).toMatch(/export function modelStepOptions\b/);
    expect(src).toMatch(/export function tierStepOptions\b/);
    expect(src).toMatch(/export function ModelSelectionPicker\b/);
  });
});
