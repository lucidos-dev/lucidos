
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { join, relative } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const SRC = join(fileURLToPath(new URL('.', import.meta.url)), '..');
const GENERATED = join(SRC, 'generated', 'thread-event-wire.ts');
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

const SCANNED = sourceFiles(SRC).filter(scanned);

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
