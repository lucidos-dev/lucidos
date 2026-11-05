import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { probeGatewayRestoreOrigin, probeRestoreOrigin, probeSelfRestore } from './restore-origin-probe.mjs';

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
    const root = mkdtempSync(join(tmpdir(), 'origin-'));
    const server = createServer((req, res) => {
      expect(req.method).toBe('GET');
      expect(req.url).toBe('/api/v1/snapshots/restore-caller');
      expect(req.headers['x-lucidos-local-token']).toBe('local-secret');
      expect(req.headers['x-lucidos-agent-origin-token']).toBe('origin-secret');
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    });
    try {
      const home = join(root, '.lucidos');
      mkdirSync(join(home, 'run'), { recursive: true });
      writeFileSync(join(home, 'local-token'), 'local-secret\n');
      const digest = createHash('sha256').update(realpathSync(root)).digest('hex').slice(0, 16);
      await new Promise<void>((resolve, reject) => {
        server.once('error', reject);
        server.listen(join(home, 'run', digest), resolve);
      });
      const env = { HOME: root, LUCIDOS_WORKSPACE: root, LUCIDOS_AGENT_ORIGIN_TOKEN: 'origin-secret', LUCIDOS_THREAD_ID: 'thread' };
      const proof = await probeRestoreOrigin(env);
      expect(proof).toEqual(verified ? { verified: true, workspace_id: 'demo', source_thread_id: 'thread' } : { verified: false });
      expect(JSON.stringify(proof)).not.toContain('secret');
      let gatewayStatus = status;
      let gatewayBody = body;
      const gateway = createServer((req, res) => {
        expect(req.method).toBe('POST');
        expect(req.url).toBe('/~/api/v1/control/workspaces/demo/snapshot-restore-caller');
        expect(req.headers['x-lucidos-local-token']).toBe('local-secret');
        let data = '';
        req.on('data', chunk => { data += chunk; });
        req.on('end', () => {
          expect(JSON.parse(data)).toEqual({ origin_token: 'origin-secret' });
          res.writeHead(gatewayStatus, { 'content-type': 'application/json' });
          res.end(JSON.stringify(gatewayBody));
        });
      });
      await new Promise<void>(resolve => gateway.listen(0, '127.0.0.1', resolve));
      try {
        const address = gateway.address() as { port: number };
        const forwarded = await probeGatewayRestoreOrigin(`http://127.0.0.1:${address.port}`, env);
        expect(forwarded).toEqual(proof);
        expect(JSON.stringify(forwarded)).not.toContain('secret');
        if (verified) {
          for (const refusal of [403, 302, 503]) {
            gatewayStatus = refusal;
            expect(await probeGatewayRestoreOrigin(`http://127.0.0.1:${address.port}`, env)).toEqual({ verified: false });
          }
          gatewayStatus = 200;
          gatewayBody = { ...body, source_thread_id: 'another-thread' };
          expect(await probeGatewayRestoreOrigin(`http://127.0.0.1:${address.port}`, env)).toEqual({ verified: false });
        }
        expect(await probeGatewayRestoreOrigin('https://remote.example', env)).toEqual({ verified: false });
      } finally {
        await new Promise<void>(resolve => gateway.close(() => resolve()));
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
      await new Promise<void>(resolve => server.close(() => resolve()));
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('refuses missing credentials and unreachable engines without exposing errors', async () => {
    expect(await probeRestoreOrigin({})).toEqual({ verified: false });
    expect(await probeRestoreOrigin({ HOME: '/absent-probe-home', LUCIDOS_WORKSPACE: '/absent-probe-workspace', LUCIDOS_AGENT_ORIGIN_TOKEN: 'secret', LUCIDOS_THREAD_ID: 'thread' })).toEqual({ verified: false });
  });
});
