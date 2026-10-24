import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { SourceMapConsumer } from 'source-map-js';
import { describe, expect, it } from 'vitest';
import { coveredSourceFiles, type LoadedMap, type ScriptCoverage } from './coverage';

// A real Chromium coverage recording of a three-module bundle: main.ts calls
// greet.ts on load, and unused.ts is bundled but never called.
const SAMPLE = fileURLToPath(new URL('./__fixtures__/coverage-sample', import.meta.url));
const ASSETS = path.join(SAMPLE, 'assets');
const scripts = JSON.parse(fs.readFileSync(path.join(SAMPLE, 'coverage.json'), 'utf8')) as ScriptCoverage[];

function sampleMap(url: string): LoadedMap | null {
  const file = path.join(ASSETS, `${path.basename(new URL(url).pathname)}.map`);
  if (!fs.existsSync(file)) return null;
  return { consumer: new SourceMapConsumer(JSON.parse(fs.readFileSync(file, 'utf8'))), dir: ASSETS };
}

describe('coveredSourceFiles', () => {
  it('names the files whose functions ran, and not a bundled file that never ran', () => {
    expect([...coveredSourceFiles(scripts, sampleMap, SAMPLE)].sort()).toEqual(['src/greet.ts', 'src/main.ts']);
  });

  it('never counts the script root, which spans every module', () => {
    const rootOnly = scripts.map((s) => ({
      ...s,
      functions: s.functions.filter((f) => f.ranges[0].startOffset === 0),
    }));
    expect(coveredSourceFiles(rootOnly, sampleMap, SAMPLE).size).toBe(0);
  });

  it('drops a script with no map, and files outside the root', () => {
    expect(coveredSourceFiles(scripts, () => null, SAMPLE).size).toBe(0);
    expect(coveredSourceFiles(scripts, sampleMap, path.join(SAMPLE, 'assets')).size).toBe(0);
  });
});
