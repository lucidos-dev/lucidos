
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
