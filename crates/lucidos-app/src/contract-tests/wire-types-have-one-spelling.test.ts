// Drift guard: a payload type the Rust source owns is spelled ONCE, in
// `thread-event-wire.ts`, and nowhere else.
//
// Why this exists: `ApiUsage` used to be written twice, as an interface in
// `store/types.ts` and as an inline object literal on the `ContextCaptured`
// union member. Two object types differing by one optional field are
// assignable both ways. So `tsc` says nothing when a new Rust field reaches
// one copy and misses the other. The type just quietly understates the
// payload, which is the exact failure the generator ends.
//
// A re-export is fine and is the point: `store/types.ts` and
// `thread-event-types.ts` both re-export from the generated file so consumers
// keep one import path. What this forbids is a second DECLARATION.

import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { join, relative } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const SRC = join(fileURLToPath(new URL('.', import.meta.url)), '..');
const GENERATED = join(SRC, 'generated', 'thread-event-wire.ts');
/** This guard, which spells the banned shapes out in its own comments. */
const SELF = fileURLToPath(import.meta.url);

function scanned(file: string): boolean {
  return file !== GENERATED && file !== SELF;
}

function sourceFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((entry: string) => {
    const path = join(dir, entry);
    if (statSync(path).isDirectory()) return sourceFiles(path);
    return /\.tsx?$/.test(entry) ? [path] : [];
  });
}

/** Walked once: both checks read the same set, and the tree has ~1000 files. */
const SCANNED = sourceFiles(SRC).filter(scanned);

/** Names the generated file declares, read from the file itself so the list
 *  cannot go stale. */
function generatedTypeNames(): string[] {
  const src = readFileSync(GENERATED, 'utf8');
  return [...src.matchAll(/^export (?:type|interface) (\w+)[\s={]/gm)].map((m) => m[1]);
}

describe('the wire payload types have exactly one spelling', () => {
  it('no file outside src/generated re-declares a generated type', () => {
    const names = generatedTypeNames();
    expect(names.length).toBeGreaterThan(20);
    const pattern = new RegExp(`^export (?:type|interface) (${names.join('|')})[\\s={]`, 'gm');

    const offenders: string[] = [];
    for (const file of SCANNED) {
      for (const m of readFileSync(file, 'utf8').matchAll(pattern)) {
        offenders.push(`${relative(SRC, file)} declares ${m[1]}`);
      }
    }
    expect(
      offenders,
      'These types are generated from Rust. Re-export them from ' +
        "'generated/thread-event-wire' instead of declaring a second copy: " +
        offenders.join('; '),
    ).toEqual([]);
  });

  it('no file re-spells the usage block as an inline object literal', () => {
    const offenders: string[] = [];
    for (const file of SCANNED) {
      const src = readFileSync(file, 'utf8');
      // An object TYPE literal carrying the usage fields, e.g.
      // `usage?: { input_tokens: number; cache_read_tokens: number; ... }`.
      // Both names are required, because the token count alone also appears
      // on the retired `ContextTokensMeasured` payload. A fixture assigning
      // values is not a type and is left alone.
      if (/\{[^}]*\binput_tokens: number\b[^}]*\bcache_read_tokens: number\b/.test(src)) {
        offenders.push(relative(SRC, file));
      }
    }
    expect(
      offenders,
      `These files spell the usage block inline instead of using \`ApiUsage\`: ${offenders.join(', ')}`,
    ).toEqual([]);
  });
});
