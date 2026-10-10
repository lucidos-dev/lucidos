/**
 * Stamp the built `dist/` with the commit it was built from.
 *
 * The engine reads this file from the snapshot it serves. The *New version
 * available* list then leaves out what that snapshot already carries
 * (`read_source_commit` in `crates/lucidos-engine/src/api/frontend_snapshot.rs`).
 *
 * HEAD is read at `buildStart`, before any source is read. If a merge lands
 * mid-build, the stamp names the older commit. The list then shows too much,
 * never too little.
 *
 * Active only under the dev build-watch (`LUCIDOS_ATOMIC_DIST`). The file goes
 * into the build's own `outDir`, the staging dir, so it publishes together with
 * the bundle it describes. See
 * `docs/plans/2026-10-10-the-served-client-names-its-commit.md`.
 */

// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import { spawnSync } from 'node:child_process';
// @ts-expect-error: same
import fs from 'node:fs';
// @ts-expect-error: same
import { resolve } from 'node:path';
import type { Plugin } from 'vite';
import { SOURCE_COMMIT_FILE } from '../../../packages/lucidos-sdk/src/generated/engine-constants';

const COMMIT_SHA = /^[0-9a-f]{40}$/;

/** `git rev-parse HEAD` in `cwd`, or `null` when git cannot answer. */
export function readHeadCommit(cwd: string): string | null {
  const out = spawnSync('git', ['rev-parse', 'HEAD'], { cwd, encoding: 'utf8' });
  if (out.status !== 0 || typeof out.stdout !== 'string') return null;
  const sha = out.stdout.trim();
  return COMMIT_SHA.test(sha) ? sha : null;
}

export function sourceCommitStamp(
  enabled: boolean,
  headCommit: () => string | null,
  defaultOutDir: string,
): Plugin {
  let commit: string | null = null;
  return {
    name: 'lucidos-source-commit-stamp',
    apply: 'build',
    buildStart() {
      commit = enabled ? headCommit() : null;
    },
    writeBundle(options) {
      if (!commit) return;
      fs.writeFileSync(resolve(options.dir ?? defaultOutDir, SOURCE_COMMIT_FILE), `${commit}\n`);
    },
  };
}
