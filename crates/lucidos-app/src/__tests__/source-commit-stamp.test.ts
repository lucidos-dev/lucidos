import { afterEach, describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import fs from 'node:fs';
// @ts-expect-error: same
import os from 'node:os';
// @ts-expect-error: same
import { dirname, join } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { SOURCE_COMMIT_FILE } from '@lucidos/engine-constants';
import { readHeadCommit, sourceCommitStamp } from '../../vite/sourceCommitStamp';

/**
 * The build stamps `dist/` with the commit it was built from. The engine reads
 * it to leave out of the switch list what the served client already carries
 * (docs/plans/2026-10-10-the-served-client-names-its-commit.md). Tested from
 * here because Vitest's `include` covers `src/` only.
 */
const SHA = 'a'.repeat(40);

type Hooks = {
  buildStart: () => void;
  writeBundle: (options: { dir?: string }) => void;
};

function runBuild(plugin: ReturnType<typeof sourceCommitStamp>, dir?: string) {
  const hooks = plugin as unknown as Hooks;
  hooks.buildStart();
  hooks.writeBundle({ dir });
}

const dirs: string[] = [];
function scratch(): string {
  const dir = fs.mkdtempSync(join(os.tmpdir(), 'source-commit-stamp-'));
  dirs.push(dir);
  return dir;
}

afterEach(() => {
  for (const dir of dirs.splice(0)) fs.rmSync(dir, { recursive: true, force: true });
});

describe('sourceCommitStamp', () => {
  it('writes the commit into the build outDir, which is the staging dir', () => {
    const staging = scratch();
    const live = scratch();
    runBuild(sourceCommitStamp(true, () => SHA, live), staging);
    expect(fs.readFileSync(join(staging, SOURCE_COMMIT_FILE), 'utf8')).toBe(`${SHA}\n`);
    expect(fs.existsSync(join(live, SOURCE_COMMIT_FILE))).toBe(false);
  });

  it('writes nothing outside the dev build-watch, so a production build is unchanged', () => {
    const out = scratch();
    let asked = false;
    runBuild(sourceCommitStamp(false, () => { asked = true; return SHA; }, out), out);
    expect(asked).toBe(false);
    expect(fs.existsSync(join(out, SOURCE_COMMIT_FILE))).toBe(false);
  });

  it('writes nothing when git cannot answer, which the engine reads as unknown', () => {
    const out = scratch();
    runBuild(sourceCommitStamp(true, () => null, out), out);
    expect(fs.existsSync(join(out, SOURCE_COMMIT_FILE))).toBe(false);
  });

  it('reads HEAD when the build starts, not when it writes', () => {
    const out = scratch();
    let head = SHA;
    const plugin = sourceCommitStamp(true, () => head, out) as unknown as Hooks;
    plugin.buildStart();
    head = 'b'.repeat(40);
    plugin.writeBundle({ dir: out });
    expect(fs.readFileSync(join(out, SOURCE_COMMIT_FILE), 'utf8')).toBe(`${SHA}\n`);
  });
});

describe('readHeadCommit', () => {
  it('answers a full sha inside a checkout', () => {
    expect(readHeadCommit(dirname(fileURLToPath(import.meta.url)))).toMatch(/^[0-9a-f]{40}$/);
  });

  it('answers null outside one', () => {
    expect(readHeadCommit(scratch())).toBeNull();
  });
});
