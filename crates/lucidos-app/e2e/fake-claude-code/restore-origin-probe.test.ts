import { createServer, type IncomingMessage, type Server } from 'node:http';
import { describe, expect, it } from 'vitest';
import { probeGatewayRestoreOrigin, probeRestoreOrigin, probeSelfRestore } from './restore-origin-probe.mjs';

async function listen(server: Server): Promise<string> {
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  return `http://127.0.0.1:${(server.address() as { port: number }).port}`;
}

const close = (server: Server) => new Promise<void>(resolve => server.close(() => resolve()));

// A sandboxed agent holds the origin token and nothing from ~/.lucidos.
function expectOriginOnly(req: IncomingMessage) {
  expect(req.headers['x-lucidos-agent-origin-token']).toBe('origin-secret');
  expect(req.headers['x-lucidos-local-token']).toBeUndefined();
}

describe('fake-agent restore origin probe', () => {
  it.each([
    [200, { workspace_id: 'demo', source_thread_id: 'thread', secret: 'response-secret' }, true],
    [403, { workspace_id: 'demo', source_thread_id: 'thread', secret: 'response-secret' }, false],
    [200, { workspace_id: 'demo', source_thread_id: 'another-thread' }, false],
    [200, { workspace_id: '../demo', source_thread_id: 'thread' }, false],
    [200, { source_thread_id: 'thread' }, false],
    [200, { workspace_id: 'demo-', source_thread_id: 'thread' }, false],
    [200, { workspace_id: 'a'.repeat(65), source_thread_id: 'thread' }, false],
    [302, { workspace_id: 'demo', source_thread_id: 'thread' }, false],
  ])('returns only bound proof for HTTP %s and %j', async (status, body, verified) => {
    const engine = createServer((req, res) => {
      expect(req.method).toBe('GET');
      expect(req.url).toBe('/api/v1/snapshots/restore-caller');
      expectOriginOnly(req);
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    });
    try {
      const env = {
        HOME: '/absent-probe-home', LUCIDOS_API_BASE_URL: await listen(engine),
        LUCIDOS_AGENT_ORIGIN_TOKEN: 'origin-secret', LUCIDOS_THREAD_ID: 'thread',
      };
      const proof = await probeRestoreOrigin(env);
      expect(proof).toEqual(verified ? { verified: true, workspace_id: 'demo', source_thread_id: 'thread' } : { verified: false });
      expect(JSON.stringify(proof)).not.toContain('secret');
      let gatewayStatus = status;
      let gatewayBody = body;
      const gateway = createServer((req, res) => {
        expect(req.method).toBe('POST');
        expect(req.url).toBe('/~/api/v1/control/workspaces/demo/snapshot-restore-caller');
        expectOriginOnly(req);
        let data = '';
        req.on('data', chunk => { data += chunk; });
        req.on('end', () => {
          expect(JSON.parse(data)).toEqual({ origin_token: 'origin-secret' });
          res.writeHead(gatewayStatus, { 'content-type': 'application/json' });
          res.end(JSON.stringify(gatewayBody));
        });
      });
      const base = await listen(gateway);
      try {
        const forwarded = await probeGatewayRestoreOrigin(base, env);
        expect(forwarded).toEqual(proof);
        expect(JSON.stringify(forwarded)).not.toContain('secret');
        if (verified) {
          for (const refusal of [403, 302, 503]) {
            gatewayStatus = refusal;
            expect(await probeGatewayRestoreOrigin(base, env)).toEqual({ verified: false });
          }
          gatewayStatus = 200;
          gatewayBody = { ...body, source_thread_id: 'another-thread' };
          expect(await probeGatewayRestoreOrigin(base, env)).toEqual({ verified: false });
        }
        expect(await probeGatewayRestoreOrigin('https://remote.example', env)).toEqual({ verified: false });
        expect(await probeGatewayRestoreOrigin(`${base}/prefix`, env)).toEqual({ verified: false });
      } finally {
        await close(gateway);
      }
      let invoked = false;
      const refusal = await probeSelfRestore(env, (command: string, args: string[], options: { env: unknown }) => {
        invoked = true;
        expect(command).toBe('lucidos');
        expect(args).toEqual(['snapshot', 'restore', 'snap_00000000000000000000000000', '--workspace', 'demo', '--expected-workspace-id', 'demo']);
        expect(options.env).toBe(env);
        return { status: 1, stdout: 'secret', stderr: 'snapshot_restore_own_workspace_refused secret' };
      });
      expect(invoked).toBe(verified);
      expect(refusal).toEqual(verified ? { ...proof, self_restore_refused: true } : proof);
      expect(JSON.stringify(refusal)).not.toContain('secret');
    } finally {
      await close(engine);
    }
  });

  it('finds a direct engine by its port when no base URL is named', async () => {
    const engine = createServer((req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify(req.url === '/api/v1/health'
        ? { status: 'ok' }
        : { workspace_id: 'demo', source_thread_id: 'thread' }));
    });
    try {
      const port = new URL(await listen(engine)).port;
      const env = { LUCIDOS_API_PORT: port, LUCIDOS_AGENT_ORIGIN_TOKEN: 'origin-secret', LUCIDOS_THREAD_ID: 'thread' };
      expect(await probeRestoreOrigin(env)).toEqual({ verified: true, workspace_id: 'demo', source_thread_id: 'thread' });
      expect(await probeRestoreOrigin({ ...env, LUCIDOS_API_PORT: 'not-a-port' })).toEqual({ verified: false });
    } finally {
      await close(engine);
    }
  });

  it('refuses missing credentials, a remote engine and unreachable engines without exposing errors', async () => {
    const env = { LUCIDOS_AGENT_ORIGIN_TOKEN: 'secret', LUCIDOS_THREAD_ID: 'thread' };
    expect(await probeRestoreOrigin({})).toEqual({ verified: false });
    expect(await probeRestoreOrigin(env)).toEqual({ verified: false });
    expect(await probeRestoreOrigin({ ...env, LUCIDOS_API_BASE_URL: 'http://remote.example' })).toEqual({ verified: false });
    expect(await probeRestoreOrigin({ ...env, LUCIDOS_API_BASE_URL: 'http://127.0.0.1:1' })).toEqual({ verified: false });
  });
});
