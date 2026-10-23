/** Per-spec frontend coverage for the main check (ADR 0502, as amended).
 *
 *  Under `LUCIDOS_E2E_COVERAGE_DIR`, each Chromium test records which source
 *  files its pages ran, through the hidden sourcemaps a coverage build emits.
 *  `/harden` reads the merged result to pick the browser specs a diff can break.
 *  Recording never changes a test's outcome: every failure here is a warning.
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { RawSourceMap, SourceMapConsumer } from 'source-map-js';
import type { BrowserContext, Page, TestInfo } from '@playwright/test';

export interface CoverageRange {
  startOffset: number;
  endOffset: number;
  count: number;
}

export interface ScriptCoverage {
  url: string;
  source?: string;
  functions: { functionName: string; ranges: CoverageRange[] }[];
}

export interface LoadedMap {
  consumer: SourceMapConsumer;
  /** The directory the map's `sources` are relative to. */
  dir: string;
}

const APP_DIR = fileURLToPath(new URL('..', import.meta.url));
const REPO_ROOT = path.resolve(APP_DIR, '../..');
const DIST_ASSETS = path.join(APP_DIR, 'dist', 'assets');

function lineStarts(source: string): number[] {
  const starts = [0];
  for (let i = 0; i < source.length; i += 1) {
    if (source.charCodeAt(i) === 10) starts.push(i + 1);
  }
  return starts;
}

/** 1-based line and 0-based column of `offset`, the shape a sourcemap asks for. */
function position(starts: number[], offset: number): { line: number; column: number } {
  let lo = 0;
  let hi = starts.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (starts[mid] <= offset) lo = mid;
    else hi = mid - 1;
  }
  return { line: lo + 1, column: offset - starts[lo] };
}

/** Repo-relative paths of the source files whose functions ran.
 *
 *  The script root is skipped: it is the whole bundle's top level, runs once
 *  per load, and spans every module in the chunk. Files outside the repo, and
 *  dependencies under node_modules, are dropped.
 */
export function coveredSourceFiles(
  scripts: ScriptCoverage[],
  loadMap: (url: string) => LoadedMap | null,
  repoRoot: string,
): Set<string> {
  const files = new Set<string>();
  for (const script of scripts) {
    const source = script.source;
    if (source === undefined) continue;
    const map = loadMap(script.url);
    if (!map) continue;
    const starts = lineStarts(source);
    for (const fn of script.functions) {
      const whole = fn.ranges[0];
      if (!whole || whole.count === 0) continue;
      if (whole.startOffset === 0 && whole.endOffset >= source.length) continue;
      const original = map.consumer.originalPositionFor(position(starts, whole.startOffset));
      if (!original.source) continue;
      const relative = path.relative(repoRoot, path.resolve(map.dir, original.source));
      if (relative.startsWith('..') || path.isAbsolute(relative)) continue;
      if (relative.split(path.sep).includes('node_modules')) continue;
      files.add(relative.split(path.sep).join('/'));
    }
  }
  return files;
}

const loadedMaps = new Map<string, LoadedMap | null>();

/** The hidden sourcemap beside a built asset, by the script's file name. */
function distMap(Consumer: typeof SourceMapConsumer, url: string): LoadedMap | null {
  let name: string;
  try {
    const { pathname } = new URL(url);
    if (!pathname.includes('/assets/')) return null;
    name = path.basename(pathname);
  } catch {
    return null;
  }
  const file = path.join(DIST_ASSETS, `${name}.map`);
  if (!loadedMaps.has(file)) {
    let loaded: LoadedMap | null = null;
    try {
      const raw = JSON.parse(fs.readFileSync(file, 'utf8')) as RawSourceMap;
      loaded = { consumer: new Consumer(raw), dir: DIST_ASSETS };
    } catch {
      // No map for this asset: a chunk the coverage build did not emit one for.
    }
    loadedMaps.set(file, loaded);
  }
  return loadedMaps.get(file) ?? null;
}

/** What one worker has recorded per spec file, so each write is the union. */
const recorded = new Map<string, Set<string>>();

export function coverageDir(): string | undefined {
  return process.env.LUCIDOS_E2E_COVERAGE_DIR || undefined;
}

export interface CoverageRecorder {
  /** Resolves once `page` records, so its first script is not missed. */
  started(page: Page): Promise<void>;
  flush(testInfo: TestInfo): Promise<void>;
}

/** Record every page `context` opens, until `flush` writes the spec's file. */
export function recordCoverage(context: BrowserContext, dir: string): CoverageRecorder {
  const starts = new Map<Page, Promise<void>>();
  const scripts: ScriptCoverage[] = [];
  const collected = new Set<Page>();
  const collect = async (page: Page) => {
    if (collected.has(page) || page.isClosed()) return;
    collected.add(page);
    try {
      await starts.get(page);
      scripts.push(...(await page.coverage.stopJSCoverage()));
    } catch (err) {
      console.warn(`[e2e coverage] could not collect a page: ${String(err)}`);
    }
  };
  const onPage = (page: Page) => {
    starts.set(
      page,
      page.coverage.startJSCoverage({ resetOnNavigation: false }).catch((err: unknown) => {
        console.warn(`[e2e coverage] could not start on a page: ${String(err)}`);
      }),
    );
    // A test that closes its own page would take the page's coverage with it.
    const close = page.close.bind(page);
    page.close = async (options) => {
      await collect(page);
      return close(options);
    };
  };
  context.on('page', onPage);
  for (const page of context.pages()) onPage(page);

  return {
    started: (page) => starts.get(page) ?? Promise.resolve(),
    async flush(testInfo) {
      context.off('page', onPage);
      try {
        for (const page of starts.keys()) await collect(page);
        const spec = path.basename(testInfo.file);
        const key = `${spec}@${testInfo.project.name}@w${testInfo.workerIndex}`;
        const files = recorded.get(key) ?? new Set<string>();
        // Loaded here, so a run without coverage never needs the package.
        const { SourceMapConsumer: Consumer } = await import('source-map-js');
        const loadMap = (url: string) => distMap(Consumer, url);
        for (const file of coveredSourceFiles(scripts, loadMap, REPO_ROOT)) files.add(file);
        recorded.set(key, files);
        fs.mkdirSync(dir, { recursive: true });
        fs.writeFileSync(
          path.join(dir, `${key}.json`),
          JSON.stringify({ spec, project: testInfo.project.name, files: [...files].sort() }),
        );
      } catch (err) {
        console.warn(`[e2e coverage] not recorded for ${testInfo.title}: ${String(err)}`);
      }
    },
  };
}
