// @vitest-environment node
// @ts-expect-error: Vitest provides Node APIs; this project has no Node typings.
import { execFileSync } from 'node:child_process';
// @ts-expect-error: Vitest provides Node APIs; this project has no Node typings.
import { fileURLToPath } from 'node:url';
import { it } from 'vitest';

it('lists registered workspace health with safe transport and clean failures', () => {
  execFileSync('python3', [fileURLToPath(new URL('./list-workspaces.fixture.py', import.meta.url))], {
    cwd: fileURLToPath(new URL('../../../../', import.meta.url)),
    stdio: 'pipe',
    timeout: 30_000,
  });
}, 30_000);
