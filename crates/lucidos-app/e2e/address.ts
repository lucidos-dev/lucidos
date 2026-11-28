import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

export const E2E_WORKSPACE = resolve(
  process.env.E2E_WORKSPACE
    ?? `${process.env.LUCIDOS_E2E_ROOT ?? `${process.env.HOME}/.lucidos/e2e-root`}/workspaces/e2e-test`,
);

export function readAddress(): { port: number; proto: string } {
  const portsFile = resolve(E2E_WORKSPACE, '.lucidos/ports');
  if (!existsSync(portsFile)) {
    throw new Error(
      `Ports file not found: ${portsFile}. Start the workspace first: ./scripts/web-dev.sh -w ${E2E_WORKSPACE} -b`,
    );
  }
  const content = readFileSync(portsFile, 'utf-8');
  const match = content.match(/VITE_PORT=(\d+)/);
  if (!match) throw new Error(`VITE_PORT not found in ${portsFile}`);
  const protos = [...content.matchAll(/^PROTO=(\w+)$/gm)];
  return {
    port: parseInt(match[1], 10),
    proto: protos.length ? protos[protos.length - 1][1] : 'https',
  };
}

export function getBaseUrl(): string {
  const { port, proto } = readAddress();
  return `${proto}://localhost:${port}`;
}
