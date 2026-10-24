/**
 * Bring the generated files up to date before Vite reads any of them.
 *
 * They are build outputs, so a fresh checkout or a merge can
 * leave them missing or stale. `codegen.sh --cached-only` copies the rendering
 * for this tree from the codegen cache, rebuilds the SDK bundles, and never
 * compiles. On a cache miss it keeps what is already there, so the build-watch
 * never waits on rustc. It fails only when nothing has been generated yet.
 *
 * A tree with no `scripts/codegen.sh` is an extracted commit, such as the
 * first-paint check's merge base. Whoever extracted it prepares its generated
 * files (`prepareGenerated` in `first-paint-growth.mjs`).
 */

// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import { spawnSync } from 'node:child_process';
// @ts-expect-error: same
import { existsSync } from 'node:fs';

export function ensureGenerated(repoRoot: string): void {
  const script = `${repoRoot}/scripts/codegen.sh`;
  if (!existsSync(script)) return;
  const run = spawnSync('bash', [script, '--cached-only'], { stdio: 'inherit' });
  if (run.status !== 0) {
    throw new Error('scripts/codegen.sh --cached-only failed (its output is above). Run: ./scripts/codegen.sh');
  }
}
