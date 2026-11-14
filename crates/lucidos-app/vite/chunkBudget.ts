/**
 * Hold each built chunk to the number for its class (ADR 0436).
 *
 * - **The entry chunk**, held to `ENTRY_BUDGET`. Its bytes sit in front of the
 *   startup fetches (ADR 0288).
 * - **The first-paint set**, held to `FIRST_PAINT_BUDGET`: the entry, the
 *   shell, and every chunk either imports statically.
 * - **A lazy chunk**, every other one, warned past `LAZY_CHUNK_LIMIT_KB`. It
 *   costs one surface's open, never a cold open, so it never fails a build.
 *
 * Past 90 % of a budget every build warns, so the nightly clean build goes red
 * days before a budget blocks an Apply.
 *
 * The dev build-watch only reports an overrun, since a failed rebuild would
 * strand every later Apply. Its one-shot builds never set `watchMode`, so it
 * names itself through `DEV_BUILD_WATCH_ENV` instead (ADR 0353).
 */

import type { Plugin } from 'vite';
import { bundleChunks, findShellChunk, type BundleChunk } from './shellChunk';

/** Set by `dev-build-watch.mjs` on its own `vite build` child, to its own pid. */
export const DEV_BUILD_WATCH_ENV = 'LUCIDOS_DEV_BUILD_WATCH';

/** The first-paint report's name in the build's output directory. Only the
 *  growth check (`first-paint-growth.mjs`, ADR 0454) asks for one. */
export const FIRST_PAINT_REPORT_FILE = 'first-paint-report.json';

/** Starts each stdout line the build-watch reads a measurement from. */
export const BUDGET_LINE_PREFIX = 'lucidos-chunk-budget ';

/** The share of a budget kept free before every build starts warning. */
export const HEADROOM_SHARE = 0.1;

export type Budget = {
  name: 'entry' | 'first-paint';
  label: string;
  kb: number;
  /** The move that brings the measurement down, named in every message. */
  fix: string;
};

export const ENTRY_BUDGET: Budget = {
  name: 'entry',
  label: 'Entry chunk',
  kb: 600,
  fix: 'Move code the first frame does not need out of the entry: see the shell chunk '
    + 'and shell startup in docs/glossary.md.',
};

export const FIRST_PAINT_BUDGET: Budget = {
  name: 'first-paint',
  label: 'First-paint set',
  kb: 1400,
  fix: 'Move code the first frame does not draw behind a dynamic import, idle-prefetched '
    + 'if its opener must not wait: see ADR 0436 and idle prefetch in docs/glossary.md.',
};

export const LAZY_CHUNK_LIMIT_KB = 400;

/** What one budget measured: a file name, or the set's file list. */
export type Measurement = { subject: string; bytes: number };

export type BudgetVerdict =
  | { kind: 'within' }
  | { kind: 'low-headroom' | 'report' | 'fail'; message: string };

export type LazyChunkVerdict = { kind: 'within' } | { kind: 'over'; message: string };

type MeasuredChunk = BundleChunk & { code: string };

/** Vite prints chunk sizes in kB of 1000 bytes; the messages use the same unit. */
function kB(bytes: number): string {
  return `${(bytes / 1000).toFixed(2)} kB`;
}

const utf8 = new TextEncoder();

/**
 * Whether this build runs under the dev build-watch.
 *
 * The variable counts only when it names this process's parent. The watcher
 * spawns Vite directly, so there it always does. Exported in a shell, or
 * inherited by an unrelated build, it names some other pid, and the build
 * stays strict.
 */
export function underDevBuildWatch(env: Record<string, string | undefined>, ppid: number): boolean {
  const named = env[DEV_BUILD_WATCH_ENV];
  return named !== undefined && named !== '' && named === String(ppid);
}

export function budgetVerdict(measured: Measurement, budget: Budget, reportOnly: boolean): BudgetVerdict {
  const what = `${budget.label} ${measured.subject}`;
  const budgetBytes = budget.kb * 1000;
  const softLineBytes = budgetBytes * (1 - HEADROOM_SHARE);
  if (measured.bytes <= softLineBytes) return { kind: 'within' };
  if (measured.bytes <= budgetBytes) {
    return {
      kind: 'low-headroom',
      message:
        `${what} is ${kB(measured.bytes)}, only ${kB(budgetBytes - measured.bytes)} under its `
        + `${budget.kb} kB budget. Past ${kB(softLineBytes)} every build warns. ${budget.fix}`,
    };
  }
  const message =
    `${what} is ${kB(measured.bytes)}, over its ${budget.kb} kB budget `
    + `(crates/lucidos-app/vite/chunkBudget.ts). Do not raise the number. ${budget.fix}`;
  if (!reportOnly) return { kind: 'fail', message };
  return {
    kind: 'report',
    message: `${budget.label.toUpperCase()} OVER BUDGET, served anyway by the dev build-watch. ${message}`,
  };
}

export function lazyChunkVerdict(chunk: { fileName: string; bytes: number }): LazyChunkVerdict {
  if (chunk.bytes <= LAZY_CHUNK_LIMIT_KB * 1000) return { kind: 'within' };
  return {
    kind: 'over',
    message:
      `Lazy chunk ${chunk.fileName} is ${kB(chunk.bytes)}, over the ${LAZY_CHUNK_LIMIT_KB} kB `
      + 'lazy chunk limit. It loads on demand, so this only warns. Split the surface it '
      + 'serves, or give a dependency only it imports its own chunk: see ADR 0436.',
  };
}

/**
 * The chunks a cold open loads before `<App/>` first renders: each entry, the
 * shell, and their static-import closure, each once. A dynamic import is not
 * followed, since it loads on demand.
 */
export function firstPaintChunks<C extends MeasuredChunk>(bundle: Record<string, C | { type: 'asset' }>): C[] {
  const chunks = bundleChunks(bundle);
  const byFile = new Map(chunks.map((c) => [c.fileName, c]));
  const reached = new Map<string, C>();
  const visit = (chunk: C | undefined) => {
    if (!chunk || reached.has(chunk.fileName)) return;
    reached.set(chunk.fileName, chunk);
    for (const file of chunk.imports) visit(byFile.get(file));
  };
  for (const entry of chunks.filter((c) => c.isEntry)) visit(entry);
  visit(findShellChunk(chunks));
  return [...reached.values()];
}

/** The line the build-watch parses into `.build-watch/status.json`. */
export function budgetLine(budget: Budget, measured: Measurement): string {
  return BUDGET_LINE_PREFIX + JSON.stringify({
    budget: budget.name,
    label: budget.label,
    subject: measured.subject,
    bytes: measured.bytes,
    budgetBytes: budget.kb * 1000,
  });
}

async function gzipLength(code: string): Promise<number> {
  const stream = new Blob([code]).stream().pipeThrough(new CompressionStream('gzip'));
  return (await new Response(stream).arrayBuffer()).byteLength;
}

/** The first-paint report the growth check reads: this build's measurement,
 *  with each chunk's gzip size summed, since each is served on its own. */
async function firstPaintReport(
  entry: MeasuredChunk | undefined,
  firstPaint: MeasuredChunk[],
  bytes: Map<MeasuredChunk, number>,
): Promise<string> {
  const gzip = await Promise.all(firstPaint.map((c) => gzipLength(c.code)));
  return `${JSON.stringify({
    version: 1,
    entry: entry ? { file: entry.fileName, bytes: bytes.get(entry)! } : null,
    firstPaint: {
      bytes: firstPaint.reduce((sum, c) => sum + bytes.get(c)!, 0),
      gzipBytes: gzip.reduce((sum, n) => sum + n, 0),
      budgetBytes: FIRST_PAINT_BUDGET.kb * 1000,
      chunks: firstPaint.map((c) => ({ file: c.fileName, bytes: bytes.get(c)! })),
    },
  }, null, 2)}\n`;
}

/** `devBuildWatch`: `underDevBuildWatch(process.env, process.ppid)`, which
 *  `vite.config.ts` reads for it. `report`: emit the first-paint report, which
 *  the growth check's wrapper config asks for. */
export function chunkBudget(devBuildWatch: boolean, report = false): Plugin {
  return {
    name: 'lucidos-chunk-budget',
    apply: 'build',
    generateBundle: {
      // After Vite's own generateBundle rewrites, so the size matches what it prints.
      order: 'post',
      handler(_options, bundle) {
        const output = bundle as unknown as Record<string, MeasuredChunk | { type: 'asset' }>;
        const chunks = bundleChunks(output);
        const bytes = new Map(chunks.map((c) => [c, utf8.encode(c.code).length]));
        const firstPaint = firstPaintChunks(output);
        const entry = firstPaint.find((c) => c.isEntry);
        const measurements: [Budget, Measurement][] = [
          ...(entry ? [[ENTRY_BUDGET, { subject: entry.fileName, bytes: bytes.get(entry)! }] as [Budget, Measurement]] : []),
          [FIRST_PAINT_BUDGET, {
            subject: `(${firstPaint.length} chunks: ${firstPaint.map((c) => c.fileName).join(', ')})`,
            bytes: firstPaint.reduce((sum, c) => sum + bytes.get(c)!, 0),
          }],
        ];

        const reportOnly = devBuildWatch || this.meta.watchMode;
        const failures: string[] = [];
        for (const [budget, measured] of measurements) {
          if (devBuildWatch) console.log(budgetLine(budget, measured));
          const verdict = budgetVerdict(measured, budget, reportOnly);
          if (verdict.kind === 'fail') failures.push(verdict.message);
          if (verdict.kind === 'report' || verdict.kind === 'low-headroom') this.warn(verdict.message);
        }

        const counted = new Set(firstPaint);
        for (const chunk of chunks) {
          if (counted.has(chunk)) continue;
          const verdict = lazyChunkVerdict({ fileName: chunk.fileName, bytes: bytes.get(chunk)! });
          if (verdict.kind === 'over') this.warn(verdict.message);
        }

        if (failures.length > 0) this.error(failures.join('\n\n'));
        if (!report) return;
        return firstPaintReport(entry, firstPaint, bytes).then((source) => {
          this.emitFile({ type: 'asset', fileName: FIRST_PAINT_REPORT_FILE, source });
        });
      },
    },
  };
}
