
// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import { spawnSync } from 'node:child_process';
// @ts-expect-error: same
import fs from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

const ENGINE_CONSTANTS = resolve(
  dirname(fileURLToPath(import.meta.url)),
  '../../../packages/lucidos-sdk/src/generated/engine-constants.ts',
);

const COMMIT_SHA = /^[0-9a-f]{40}$/;

export function sourceCommitFile(): string {
  const text = fs.readFileSync(ENGINE_CONSTANTS, 'utf8');
  const found = /^export const SOURCE_COMMIT_FILE = "([^"]+)";$/m.exec(text);
  if (!found) throw new Error(`${ENGINE_CONSTANTS} no longer exports SOURCE_COMMIT_FILE as a string`);
  return found[1];
}

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
