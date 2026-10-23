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
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

/** The generated copy of the engine constants. */
const ENGINE_CONSTANTS = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../../../packages/lucidos-sdk/src/generated/engine-constants.ts',
);

const COMMIT_SHA = /^[0-9a-f]{40}$/;

/**
 * `SOURCE_COMMIT_FILE` from the generated engine constants, read as text.
 *
 * Vite bundles its config, imports included, before the config's first line
 * runs. That first line is what writes the generated files on a fresh
 * checkout, so the config cannot import one.
 */
export function sourceCommitFile(): string {
  const text = fs.readFileSync(ENGINE_CONSTANTS, 'utf8');
  const found = /^export const SOURCE_COMMIT_FILE = "([^"]+)";$/m.exec(text);
  if (!found) throw new Error(`${ENGINE_CONSTANTS} no longer exports SOURCE_COMMIT_FILE as a string`);
  return found[1];
}

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
      fs.writeFileSync(resolve(options.dir ?? defaultOutDir, sourceCommitFile()), `${commit}\n`);
    },
  };
}
