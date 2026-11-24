import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
import { REMOTE_START_SETTINGS } from './control';
import { REMOTE_START_OPTIONS } from '../../components/settings/GatewayPairingSections';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');
const REMOTE_START_RS = 'crates/lucidos-gateway-stamp/src/remote_start.rs';

function rustVariants(): string[] {
  const source: string = readFileSync(resolve(REPO_ROOT, REMOTE_START_RS), 'utf8');
  const decl = source.indexOf('pub enum RemoteStartSetting {');
  expect(decl, `RemoteStartSetting not found in ${REMOTE_START_RS}`).toBeGreaterThan(-1);
  const body = source.slice(decl, source.indexOf('}', decl));
  return [...body.matchAll(/^\s*([A-Z][A-Za-z]*),\s*$/gm)].map((m) => m[1].toLowerCase());
}

describe('RemoteStartSetting', () => {
  it('matches the Rust enum', () => {
    expect([...REMOTE_START_SETTINGS].sort()).toEqual(rustVariants().sort());
  });

  it('offers every setting once in the Settings select', () => {
    expect(REMOTE_START_OPTIONS.map((o) => o.value)).toEqual([...REMOTE_START_SETTINGS]);
  });
});
