import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

/**
 * The one reader of the workspace's own address.
 *
 * The workspace records both halves in `.lucidos/ports`, and the protocol is
 * the half that gets forgotten. `detect_tls` (scripts/lib/workspace.sh) serves
 * plain HTTP on a machine with no `.certs/`, which is every coding-agent
 * worktree, since `.certs/` is gitignored. A hardcoded `https://` then fails
 * with a TLS record error that reads like a broken server.
 *
 * Every reader of the address comes through here, so no caller can pick up the
 * port and miss the protocol.
 */
export const E2E_WORKSPACE = resolve(
  process.env.E2E_WORKSPACE ?? `${process.env.HOME}/workspaces/e2e-test`,
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
  // Later lines win, since detect_tls appends.
  const protos = [...content.matchAll(/^PROTO=(\w+)$/gm)];
  return {
    port: parseInt(match[1], 10),
    proto: protos.length ? protos[protos.length - 1][1] : 'https',
  };
}

/** The workspace's user-facing origin, protocol included. */
export function getBaseUrl(): string {
  const { port, proto } = readAddress();
  return `${proto}://localhost:${port}`;
}
