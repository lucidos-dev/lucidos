/**
 * Hold the entry chunk to its first-paint budget, `build.chunkSizeWarningLimit`.
 *
 * Rollup's own advisory only prints, so the entry chunk rotted past it for weeks
 * while every build exited 0. This turns the same number into a failure for the
 * entry chunk alone. The other chunks load on demand and keep the advisory.
 *
 * Under `vite build --watch` it reports instead of failing. The dev build-watch
 * serves the workspace, and a failed rebuild would strand every later Apply
 * behind a budget overrun. Every single-shot build fails: `/harden`, e2e, the
 * nightly clean build and the release.
 *
 * What the entry chunk may hold, and why: ADR 0288.
 */

import type { Plugin } from 'vite';

export type EntryChunkVerdict =
  | { kind: 'within' }
  | { kind: 'report' | 'fail'; message: string };

/** Vite prints chunk sizes in kB of 1000 bytes; the message uses the same unit. */
function kB(bytes: number): string {
  return `${(bytes / 1000).toFixed(2)} kB`;
}

export function entryChunkVerdict(
  chunk: { fileName: string; bytes: number },
  budgetKb: number,
  watchMode: boolean,
): EntryChunkVerdict {
  if (chunk.bytes <= budgetKb * 1000) return { kind: 'within' };
  const message =
    `Entry chunk ${chunk.fileName} is ${kB(chunk.bytes)}, over its ${budgetKb} kB first-paint `
    + 'budget (build.chunkSizeWarningLimit in crates/lucidos-app/vite.config.ts). '
    + 'Do not raise the number. Move code the first frame does not need behind a dynamic '
    + 'import: see the shell chunk and idle prefetch in docs/glossary.md.';
  return { kind: watchMode ? 'report' : 'fail', message };
}

export function entryChunkBudget(): Plugin {
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
          const bytes = new TextEncoder().encode(output.code).length;
          const verdict = entryChunkVerdict({ fileName: output.fileName, bytes }, budgetKb, this.meta.watchMode);
          if (verdict.kind === 'fail') this.error(verdict.message);
          if (verdict.kind === 'report') this.warn(verdict.message);
        }
      },
    },
  };
}
