/**
 * Hold the entry chunk to its first-paint budget, `build.chunkSizeWarningLimit`.
 *
 * Rollup's own advisory only prints, so the entry chunk rotted past it for weeks
 * while every build exited 0. This turns the same number into a failure for the
 * entry chunk alone. The other chunks load on demand and keep the advisory.
 *
 * Past 90 % of the budget, every build warns. The nightly clean build fails on
 * any warning, so creep shows there days before it blocks an Apply.
 *
 * The dev build-watch only reports an overrun. It serves the workspace, and a
 * failed rebuild would strand every later Apply behind the overrun. It runs a
 * fresh one-shot build per change, so `watchMode` never says so: the watcher
 * names itself through `DEV_BUILD_WATCH_ENV` instead. `vite build --watch` reports
 * too. Every other single-shot build fails.
 *
 * What the entry chunk may hold, and why: ADR 0288.
 */

import type { Plugin } from 'vite';

/** Set by `dev-build-watch.mjs` on its own `vite build` child, to its own pid. */
export const DEV_BUILD_WATCH_ENV = 'LUCIDOS_DEV_BUILD_WATCH';

/** Starts the one stdout line the build-watch reads the measurement from. */
export const ENTRY_CHUNK_LINE_PREFIX = 'lucidos-entry-chunk ';

/** The share of the budget kept free before every build starts warning. */
export const HEADROOM_SHARE = 0.1;

export type EntryChunkVerdict =
  | { kind: 'within' }
  | { kind: 'low-headroom' | 'report' | 'fail'; message: string };

/** Vite prints chunk sizes in kB of 1000 bytes; the message uses the same unit. */
function kB(bytes: number): string {
  return `${(bytes / 1000).toFixed(2)} kB`;
}

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

export function entryChunkVerdict(
  chunk: { fileName: string; bytes: number },
  budgetKb: number,
  reportOnly: boolean,
): EntryChunkVerdict {
  const budgetBytes = budgetKb * 1000;
  const softLineBytes = budgetBytes * (1 - HEADROOM_SHARE);
  if (chunk.bytes <= softLineBytes) return { kind: 'within' };
  if (chunk.bytes <= budgetBytes) {
    return {
      kind: 'low-headroom',
      message:
        `Entry chunk ${chunk.fileName} is ${kB(chunk.bytes)}, only ${kB(budgetBytes - chunk.bytes)} `
        + `under its ${budgetKb} kB first-paint budget. Past ${kB(softLineBytes)} every build warns. `
        + 'Move code the first frame does not need behind a dynamic import before the budget '
        + 'fails the build: see the shell chunk and shell startup in docs/glossary.md.',
    };
  }
  const message =
    `Entry chunk ${chunk.fileName} is ${kB(chunk.bytes)}, over its ${budgetKb} kB first-paint `
    + 'budget (build.chunkSizeWarningLimit in crates/lucidos-app/vite.config.ts). '
    + 'Do not raise the number. Move code the first frame does not need behind a dynamic '
    + 'import: see the shell chunk and idle prefetch in docs/glossary.md.';
  if (!reportOnly) return { kind: 'fail', message };
  return {
    kind: 'report',
    message: `ENTRY CHUNK OVER BUDGET, served anyway by the dev build-watch. ${message}`,
  };
}

/** The line the build-watch parses into `.build-watch/status.json`. */
export function entryChunkLine(chunk: { fileName: string; bytes: number }, budgetKb: number): string {
  return ENTRY_CHUNK_LINE_PREFIX
    + JSON.stringify({ fileName: chunk.fileName, bytes: chunk.bytes, budgetBytes: budgetKb * 1000 });
}

/** `devBuildWatch`: `underDevBuildWatch(process.env, process.ppid)`, which
 *  `vite.config.ts` reads for it. */
export function entryChunkBudget(devBuildWatch: boolean): Plugin {
  let budgetKb = 0;
  return {
    name: 'lucidos-entry-chunk-budget',
    apply: 'build',
    configResolved(config) {
      budgetKb = config.build.chunkSizeWarningLimit;
    },
    generateBundle: {
      // After Vite's own generateBundle rewrites, so the size matches what it prints.
      order: 'post',
      handler(_options, bundle) {
        for (const output of Object.values(bundle)) {
          if (output.type !== 'chunk' || !output.isEntry) continue;
          const chunk = { fileName: output.fileName, bytes: new TextEncoder().encode(output.code).length };
          if (devBuildWatch) console.log(entryChunkLine(chunk, budgetKb));
          const verdict = entryChunkVerdict(chunk, budgetKb, devBuildWatch || this.meta.watchMode);
          if (verdict.kind === 'fail') this.error(verdict.message);
          if (verdict.kind === 'report' || verdict.kind === 'low-headroom') this.warn(verdict.message);
        }
      },
    },
  };
}
