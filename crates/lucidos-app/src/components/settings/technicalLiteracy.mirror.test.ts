import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
import { preferences } from '../../store/store';
import {
  currentTechnicalLiteracy,
  TECHNICAL_LITERACY_LEVELS,
  TECHNICAL_LITERACY_NOT_SET,
} from '../../store/actions/preferences';
import { technicalLiteracyOptions } from './ResponseStylesSection';

/** The engine's list of *technical literacy* levels.
 *
 *  A level the picker offers but the engine lacks is stored and then ignored,
 *  so the user picks "Developer" and nothing changes. Read the Rust source,
 *  do not restate it, as `responseStyle.mirror.test.ts` does. */
const HERE = dirname(fileURLToPath(import.meta.url));
const RUST: string = readFileSync(
  resolve(HERE, '../../../../lucidos-engine/src/core/technical_literacy.rs'),
  'utf8',
);

function rustIds(): string[] {
  const match = RUST.match(/pub const IDS: &\[&str\] = &\[([^\]]*)\];/);
  if (!match) throw new Error('IDS is not declared in core/technical_literacy.rs');
  return [...match[1].matchAll(/"([a-z-]+)"/g)].map((m) => m[1]);
}

/** One `Self::X => "text"` arm per level, read out of a named Rust fn. */
function rustCopy(fnName: string): string[] {
  const body = RUST.split(`pub fn ${fnName}(self)`)[1]?.split('\n    }\n')[0];
  if (!body) throw new Error(`${fnName} is not declared in core/technical_literacy.rs`);
  return [...body.matchAll(/=> "([^"]+)"/g)].map((m) => m[1]);
}

describe('the technical literacy levels mirror the engine', () => {
  it('agrees on every level, in order', () => {
    expect(rustIds().length).toBeGreaterThan(0);
    expect([...TECHNICAL_LITERACY_LEVELS]).toEqual(rustIds());
  });

  it('agrees on the value that clears the level', () => {
    const match = RUST.match(/pub const NOT_SET_ID: &str = "([a-z-]+)";/);
    expect(match?.[1]).toBe(TECHNICAL_LITERACY_NOT_SET);
  });

  it('shows exactly the card copy the first-run setup shows', () => {
    const levels = technicalLiteracyOptions().slice(1);
    expect(levels.map((o) => o.label)).toEqual(rustCopy('card_label'));
    expect(levels.map((o) => o.description)).toEqual(rustCopy('card_line'));
  });

  it('reads the merged everyday level as Keep it plain', () => {
    preferences.value = { status: 'loaded', data: { technical_literacy: 'everyday' } };
    expect(currentTechnicalLiteracy()).toBe('non-technical');
    preferences.value = { status: 'loaded', data: { technical_literacy: 'wizard' } };
    expect(currentTechnicalLiteracy()).toBeNull();
    preferences.value = { status: 'not-loaded' };
  });

  it('offers Not set first, then every level once', () => {
    const values = technicalLiteracyOptions().map((o) => o.value);
    expect(values).toEqual([TECHNICAL_LITERACY_NOT_SET, ...rustIds()]);
    for (const option of technicalLiteracyOptions()) {
      expect(option.label).not.toBe('');
      expect(option.description).toBeTruthy();
    }
  });
});
