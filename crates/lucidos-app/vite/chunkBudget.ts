
import type { Plugin } from 'vite';
import { bundleChunks, findShellChunk, type BundleChunk } from './shellChunk';

export const DEV_BUILD_WATCH_ENV = 'LUCIDOS_DEV_BUILD_WATCH';

export const FIRST_PAINT_REPORT_FILE = 'first-paint-report.json';

export const BUDGET_LINE_PREFIX = 'lucidos-chunk-budget ';

export const HEADROOM_SHARE = 0.1;

export type Budget = {
  name: 'entry' | 'first-paint';
  label: string;
  kb: number;
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
  kb: 1950,
  fix: 'Move code the first frame does not draw behind a dynamic import, idle-prefetched '
    + 'if its opener must not wait: see ADR 0436 and idle prefetch in docs/glossary.md.',
};

export const LAZY_CHUNK_LIMIT_KB = 400;

export type Measurement = { subject: string; bytes: number };

export type BudgetVerdict =
  | { kind: 'within' }
  | { kind: 'low-headroom' | 'report' | 'fail'; message: string };

export type LazyChunkVerdict = { kind: 'within' } | { kind: 'over'; message: string };

type MeasuredChunk = BundleChunk & { code: string };

function kB(bytes: number): string {
  return `${(bytes / 1000).toFixed(2)} kB`;
}

const utf8 = new TextEncoder();

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

export function chunkBudget(devBuildWatch: boolean, report = false): Plugin {
  return {
    name: 'lucidos-chunk-budget',
    apply: 'build',
    generateBundle: {
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
