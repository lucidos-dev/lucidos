// Source-authority fixture, run INSIDE the engine-spawned fake agent. Never print
// credentials, raw responses or exceptions: even a refusal body is untrusted.
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { readFileSync, realpathSync } from 'node:fs';
import { request } from 'node:http';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export async function probeRestoreOrigin(env = process.env) {
  try {
    if (!env.HOME || !env.LUCIDOS_WORKSPACE || !env.LUCIDOS_AGENT_ORIGIN_TOKEN || !env.LUCIDOS_THREAD_ID) return { verified: false };
    // Mirrors lucidos-engine-socket::path_for and lucidos-local-token::read.
    // The live integration test checks this against the real engine listener.
    const digest = createHash('sha256').update(realpathSync(env.LUCIDOS_WORKSPACE)).digest('hex').slice(0, 16);
    const home = join(env.HOME, '.lucidos');
    const local = readFileSync(join(home, 'local-token'), 'utf8').trim();
    if (!local) return { verified: false };
    return await new Promise((resolve) => {
      const req = request({
        socketPath: join(home, 'run', digest),
        path: '/api/v1/snapshots/restore-caller', method: 'GET',
        headers: { 'x-lucidos-local-token': local, 'x-lucidos-agent-origin-token': env.LUCIDOS_AGENT_ORIGIN_TOKEN },
      }, (res) => {
        let body = '';
        res.setEncoding('utf8');
        res.on('error', () => resolve({ verified: false }));
        res.on('data', (chunk) => {
          body += chunk;
          if (body.length > 4096) req.destroy(new Error('probe response too large'));
        });
        res.on('end', () => {
          try {
            const proof = JSON.parse(body);
            if (res.statusCode !== 200 || typeof proof.workspace_id !== 'string'
              || proof.workspace_id.length > 64 || !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(proof.workspace_id)
              || proof.source_thread_id !== env.LUCIDOS_THREAD_ID) return resolve({ verified: false });
            resolve({ verified: true, workspace_id: proof.workspace_id, source_thread_id: proof.source_thread_id });
          } catch { resolve({ verified: false }); }
        });
      });
      const deadline = setTimeout(() => req.destroy(new Error('probe timed out')), 5000);
      req.on('close', () => clearTimeout(deadline));
      req.on('error', () => resolve({ verified: false }));
      req.end();
    });
  } catch { return { verified: false }; }
}

// The inherited origin token travels from the child straight to the real
// control route, never through a test-process capture endpoint or transcript.
export async function probeGatewayRestoreOrigin(base, env = process.env) {
  try {
    const url = new URL(base);
    if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || url.username || url.password
      || url.pathname !== '/' || url.search || url.hash) return { verified: false };
    const source = await probeRestoreOrigin(env);
    if (!source.verified) return source;
    const local = readFileSync(join(env.HOME, '.lucidos', 'local-token'), 'utf8').trim();
    url.pathname = `/~/api/v1/control/workspaces/${source.workspace_id}/snapshot-restore-caller`;
    return await new Promise(resolve => {
      const req = request(url, {
        method: 'POST', headers: { 'x-lucidos-local-token': local, 'content-type': 'application/json' },
      }, res => {
        let body = '';
        res.setEncoding('utf8');
        res.on('error', () => resolve({ verified: false }));
        res.on('data', chunk => {
          body += chunk;
          if (body.length > 4096) req.destroy(new Error('probe response too large'));
        });
        res.on('end', () => {
          try {
            const proof = JSON.parse(body);
            resolve(res.statusCode === 200 && proof.workspace_id === source.workspace_id
              && proof.source_thread_id === source.source_thread_id ? source : { verified: false });
          } catch { resolve({ verified: false }); }
        });
      });
      const deadline = setTimeout(() => req.destroy(new Error('probe timed out')), 5000);
      req.on('close', () => clearTimeout(deadline));
      req.on('error', () => resolve({ verified: false }));
      req.end(JSON.stringify({ origin_token: env.LUCIDOS_AGENT_ORIGIN_TOKEN }));
    });
  } catch { return { verified: false }; }
}

// Exercises source authorization only; no target is restored or even opened.
export async function probeLiveRestoreApproval(base, env = process.env) {
  try {
    const source = await probeGatewayRestoreOrigin(base, env);
    if (!source.verified) return { verified: false };
    const digest = createHash('sha256').update(realpathSync(env.LUCIDOS_WORKSPACE)).digest('hex').slice(0, 16);
    const local = readFileSync(join(env.HOME, '.lucidos', 'local-token'), 'utf8').trim();
    const headers = { 'x-lucidos-local-token': local, 'content-type': 'application/json' };
    const unix = { socketPath: join(env.HOME, '.lucidos', 'run', digest),
      headers: { ...headers, 'x-lucidos-agent-origin-token': env.LUCIDOS_AGENT_ORIGIN_TOKEN } };
    const call = (destination, body, dropReply = false) => new Promise(resolve => {
      const req = request(destination, res => {
        // Deliberately discard the spend response body after the source has
        // committed. Never return its proof to the caller.
        if (dropReply) { res.destroy(); resolve({ status: 0 }); return; }
        let text = '';
        res.setEncoding('utf8');
        res.on('data', chunk => { text += chunk; if (text.length > 16384) req.destroy(); });
        res.on('error', () => resolve({ status: 0 }));
        res.on('end', () => {
          try { resolve({ status: res.statusCode, body: JSON.parse(text) }); }
          catch { resolve({ status: 0 }); }
        });
      });
      const timer = setTimeout(() => req.destroy(), 20000);
      req.on('close', () => clearTimeout(timer));
      req.on('error', () => resolve({ status: 0 }));
      req.end(JSON.stringify(body));
    });
    const exact = { target_workspace_id: 'restore-probe-target', snapshot_handle: 'snap_00000000000000000000000000' };
    const spend = (approval, dropReply = false) => {
      const url = new URL(base);
      url.pathname = `/~/api/v1/control/workspaces/${source.workspace_id}/snapshot-restore-caller`;
      return call({ hostname: url.hostname, port: url.port, path: url.pathname, method: 'POST', headers },
        { origin_token: env.LUCIDOS_AGENT_ORIGIN_TOKEN, approval }, dropReply);
    };
    for (const decision of ['allow', 'deny', 'lost']) {
      const requested = await call({ ...unix, path: '/api/v1/owner-approvals', method: 'POST' },
        { verb: 'restore-snapshot', ...exact, reason: `Disposable source approval probe: ${decision}` });
      if (requested.status !== 200 || typeof requested.body?.request_id !== 'string') return { verified: false };
      const asked = await call({ ...unix, path: '/api/v1/internal/ask-user-question', method: 'POST' }, {
        thread_id: env.LUCIDOS_THREAD_ID, session_id: 'restore-approval-probe',
        tool_use_id: `restore-approval-${decision}`, questions: [{ question: requested.body.request_id,
          header: 'Approval', options: [{ label: 'Yes' }, { label: 'No' }] }],
      });
      if (asked.status !== 200 || asked.body?.refusal) return { verified: false };
      if (decision === 'allow') {
        if ((await spend({ ...exact, target_workspace_id: 'wrong-target' })).status !== 403
          || (await spend({ ...exact, snapshot_handle: 'snap_00000000000000000000000001' })).status !== 403
          || (await spend(exact)).status !== 200
          || (await spend(exact)).status !== 403) return { verified: false };
      } else if (decision === 'lost') {
        if ((await spend(exact, true)).status !== 0 || (await spend(exact)).status !== 403) return { verified: false };
      } else if ((await spend(exact)).status !== 403) return { verified: false };
    }
    return { ...source, approval_matrix: true };
  } catch { return { verified: false }; }
}

export async function probeSelfRestore(env = process.env, run = spawnSync) {
  const proof = await probeRestoreOrigin(env);
  if (!proof.verified) return proof;
  // Target ONLY the workspace independently proved by the issuing engine.
  // The CLI must refuse this before discovering a gateway or issuing any POST.
  const result = run('lucidos', [
    'snapshot', 'restore', 'snap_00000000000000000000000000',
    '--workspace', proof.workspace_id, '--expected-workspace-id', proof.workspace_id,
  ], { env, encoding: 'utf8', timeout: 10000, maxBuffer: 8192 });
  return { ...proof, self_restore_refused: result.status !== null && result.status !== 0
    && result.stderr.includes('snapshot_restore_own_workspace_refused') };
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const gateway = process.argv.indexOf('--gateway');
  const proof = gateway >= 0 ? await (process.argv.includes('--approval') ? probeLiveRestoreApproval : probeGatewayRestoreOrigin)(process.argv[gateway + 1])
    : process.argv.includes('--self-restore') ? await probeSelfRestore() : await probeRestoreOrigin();
  process.stdout.write(`${JSON.stringify(proof)}\n`);
}
