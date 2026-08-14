import { describe, it, expect } from 'vitest';
// @ts-expect-error Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error same
import { fileURLToPath } from 'node:url';
// @ts-expect-error same
import { dirname, resolve, relative } from 'node:path';

/**
 * Every Lucidos Agent reasoning picker must ask the REGISTRY what a model
 * supports, via `reasoningLevelsFor` / `clampEffortFor` in
 * `store/actions/models.ts`, never `availableReasoningLevels` /
 * `clampReasoningEffort` from `store/models.ts` directly.
 *
 * The two are not interchangeable. The wrappers look up the model's
 * `reasoning_efforts` from `/models`, which the engine derives from the row's
 * PROVIDER (`llm::reasoning::supported_efforts`) and which
 * `RoutingProvider::effort_for_model` clamps the actual request onto. The raw
 * functions fall back to an id-shape heuristic that cannot know which server
 * serves the model, and calling one directly silently reintroduces exactly the
 * drift that broke a local-model turn on 2026-08-12: the picker offered `max`,
 * the wire sent something the local server rejected, and both layers were
 * "correct" by their own rule.
 *
 * A source scan rather than a render test, matching `skeleton-guard.test.ts`:
 * the invariant is which function a call site reaches for, and the failure mode
 * is a NEW surface added later, which no existing render test would cover.
 */

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..'); // crates/lucidos-app/src

/** The wrappers' own home: it is the one place allowed to call the raw pair. */
const WRAPPER_HOME = resolve(SRC, 'store/actions/models.ts');

/** The raw, heuristic-capable functions, imported from `store/models`. */
const RAW_IMPORT =
  /import\s*\{[^}]*\b(availableReasoningLevels|clampReasoningEffort)\b[^}]*\}\s*from\s*['"][^'"]*\/models['"]/;

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

describe('reasoning pickers go through the model registry', () => {
  it('only the wrapper module imports the raw heuristic-capable pair', () => {
    const offenders = sourceFiles(SRC)
      .filter((f) => f !== WRAPPER_HOME)
      .filter((f) => RAW_IMPORT.test(readFileSync(f, 'utf8')))
      .map((f) => relative(SRC, f));

    expect(
      offenders,
      'Import `reasoningLevelsFor` / `clampEffortFor` from `store/actions/models` instead, '
        + 'so the picker offers what the engine will actually send.',
    ).toEqual([]);
  });

  it('the wrapper module still exports both wrappers', () => {
    // Guards the guard: if the wrappers were renamed away, the scan above would
    // pass vacuously while every surface went back to the heuristic.
    const src = readFileSync(WRAPPER_HOME, 'utf8');
    expect(src).toMatch(/export function reasoningLevelsFor\b/);
    expect(src).toMatch(/export function clampEffortFor\b/);
  });
});
