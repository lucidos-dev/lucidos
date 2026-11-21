/**
 * The gateway's pairing rows are hand-typed, so the remote start setting is a
 * mirror of the Rust enum. This test reads the Rust source and fails when a
 * variant is added on one side only.
 */
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

/** The variants of `pub enum RemoteStartSetting`, in their kebab-case wire form. */
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
