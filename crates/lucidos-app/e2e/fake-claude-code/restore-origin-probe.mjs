// Source-authority fixture, run INSIDE the engine-spawned fake agent. Never print
// credentials, raw responses or exceptions: even a refusal body is untrusted.
// Every hop carries the origin token alone, as `lucidos snapshot restore` does.
// Nothing here reads ~/.lucidos, which the agent sandbox hides.
import { spawnSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { request as httpRequest } from 'node:http';
import { request as httpsRequest } from 'node:https';
import { pathToFileURL } from 'node:url';

const ORIGIN_HEADER = 'x-lucidos-agent-origin-token';

// The loopback URL of `path` under `base`, or null for anything else. The
// engine names its own scheme in LUCIDOS_API_BASE_URL, so both are accepted.
function loopbackUrl(base, path) {
  try {
    const url = new URL(base);
    if (!['http:', 'https:'].includes(url.protocol) || !['127.0.0.1', 'localhost'].includes(url.hostname)
      || url.username || url.password || url.search || url.hash) return null;
    url.pathname = `${url.pathname.replace(/\/+$/, '')}${path}`;
    return url;
  } catch { return null; }
}

// One JSON request. Resolves `{ status, body }`, or `{ status: 0 }` on any fault.
function call(url, method, env, body, { dropReply = false, timeoutMs = 20000 } = {}) {
  return new Promise(resolve => {
    const headers = { [ORIGIN_HEADER]: env.LUCIDOS_AGENT_ORIGIN_TOKEN };
    if (body !== undefined) headers['content-type'] = 'application/json';
    // A loopback engine's TLS cert is self-signed, as the CLI also accepts.
    const req = url.protocol === 'https:'
      ? httpsRequest(url, { method, headers, rejectUnauthorized: false }, reply)
      : httpRequest(url, { method, headers }, reply);
    function reply(res) {
      // Deliberately discard a spend reply after the source has committed.
      // Never return its proof to the caller.
      if (dropReply) { res.destroy(); resolve({ status: 0 }); return; }
      let text = '';
      res.setEncoding('utf8');
      res.on('data', chunk => { text += chunk; if (text.length > 16384) req.destroy(); });
      res.on('error', () => resolve({ status: 0 }));
      res.on('end', () => {
        try { resolve({ status: res.statusCode, body: JSON.parse(text) }); }
        catch { resolve({ status: 0 }); }
      });
    }
    const timer = setTimeout(() => req.destroy(), timeoutMs);
    req.on('close', () => clearTimeout(timer));
    req.on('error', () => resolve({ status: 0 }));
    req.end(body === undefined ? undefined : JSON.stringify(body));
  });
}

// The engine's loopback base. Under a gateway the engine names it. A direct
// engine, as the e2e harness starts one, names only its port, and serves
// whichever of http and https its certificates allow.
async function engineBase(env) {
  if (env.LUCIDOS_API_BASE_URL) return env.LUCIDOS_API_BASE_URL;
  if (!/^[0-9]{1,5}$/.test(env.LUCIDOS_API_PORT ?? '')) return undefined;
  for (const scheme of ['http', 'https']) {
    const base = `${scheme}://127.0.0.1:${env.LUCIDOS_API_PORT}`;
    if ((await call(loopbackUrl(base, '/api/v1/health'), 'GET', env, undefined, { timeoutMs: 2000 })).status !== 0) return base;
  }
  return undefined;
}

// The fixed slug of the step that refused, so a failed run says where. It never
// carries a credential, a response or an exception.
let refusedAt = 'unknown';
const refuse = stage => { refusedAt = stage; return { verified: false }; };

export async function probeRestoreOrigin(env = process.env) {
  try {
    if (!env.LUCIDOS_AGENT_ORIGIN_TOKEN || !env.LUCIDOS_THREAD_ID) return refuse('env');
    const url = loopbackUrl(await engineBase(env), '/api/v1/snapshots/restore-caller');
    if (!url) return refuse('engine-base');
    const { status, body: proof } = await call(url, 'GET', env, undefined, { timeoutMs: 5000 });
    if (status !== 200) return refuse(`engine-status-${status}`);
    if (typeof proof?.workspace_id !== 'string'
      || proof.workspace_id.length > 64 || !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(proof.workspace_id)
      || proof.source_thread_id !== env.LUCIDOS_THREAD_ID) return refuse('engine-proof');
    return { verified: true, workspace_id: proof.workspace_id, source_thread_id: proof.source_thread_id };
  } catch { return refuse('engine-fault'); }
}

// The gateway's own control route, admitted on the origin token alone.
function restoreCallerUrl(base, workspace) {
  try {
    if (new URL(base).pathname !== '/') return null;
  } catch { return null; }
  return loopbackUrl(base, `/~/api/v1/control/workspaces/${workspace}/snapshot-restore-caller`);
}

// The inherited origin token travels from the child straight to the real
// control route, never through a test-process capture endpoint or transcript.
export async function probeGatewayRestoreOrigin(base, env = process.env) {
  try {
    const source = await probeRestoreOrigin(env);
    if (!source.verified) return source;
    const url = restoreCallerUrl(base, source.workspace_id);
    if (!url) return refuse('gateway-url');
    const { status, body: proof } = await call(url, 'POST', env,
      { origin_token: env.LUCIDOS_AGENT_ORIGIN_TOKEN }, { timeoutMs: 5000 });
    if (status !== 200) return refuse(`gateway-status-${status}`);
    return proof?.workspace_id === source.workspace_id
      && proof?.source_thread_id === source.source_thread_id ? source : refuse('gateway-proof');
  } catch { return refuse('gateway-fault'); }
}

// Exercises source authorization only; no target is restored or even opened.
export async function probeLiveRestoreApproval(base, env = process.env) {
  try {
    const source = await probeGatewayRestoreOrigin(base, env);
    if (!source.verified) return source;
    const engineRoot = await engineBase(env);
    const engine = path => loopbackUrl(engineRoot, path);
    const exact = { target_workspace_id: 'restore-probe-target', snapshot_handle: 'snap_00000000000000000000000000' };
    const spend = (approval, dropReply = false) => call(restoreCallerUrl(base, source.workspace_id), 'POST', env,
      { origin_token: env.LUCIDOS_AGENT_ORIGIN_TOKEN, approval }, { dropReply });
    for (const decision of ['allow', 'deny', 'lost']) {
      const requested = await call(engine('/api/v1/owner-approvals'), 'POST', env,
        { verb: 'restore-snapshot', ...exact, reason: `Disposable source approval probe: ${decision}` });
      if (requested.status !== 200 || typeof requested.body?.request_id !== 'string') return refuse(`approval-request-${decision}-${requested.status}`);
      const asked = await call(engine('/api/v1/internal/ask-user-question'), 'POST', env, {
        thread_id: env.LUCIDOS_THREAD_ID, session_id: 'restore-approval-probe',
        tool_use_id: `restore-approval-${decision}`, questions: [{ question: requested.body.request_id,
          header: 'Approval', options: [{ label: 'Yes' }, { label: 'No' }] }],
      });
      if (asked.status !== 200 || asked.body?.refusal) return refuse(`approval-card-${decision}-${asked.status}`);
      if (decision === 'allow') {
        const wrongTarget = await spend({ ...exact, target_workspace_id: 'wrong-target' });
        const wrongHandle = await spend({ ...exact, snapshot_handle: 'snap_00000000000000000000000001' });
        const first = await spend(exact);
        const second = await spend(exact);
        if (wrongTarget.status !== 403 || wrongHandle.status !== 403 || first.status !== 200 || second.status !== 403) {
          return refuse(`approval-allow-${wrongTarget.status}-${wrongHandle.status}-${first.status}-${second.status}`);
        }
      } else if (decision === 'lost') {
        const dropped = await spend(exact, true);
        const retry = await spend(exact);
        if (dropped.status !== 0 || retry.status !== 403) return refuse(`approval-lost-${dropped.status}-${retry.status}`);
      } else {
        const denied = await spend(exact);
        if (denied.status !== 403) return refuse(`approval-deny-${denied.status}`);
      }
    }
    return { ...source, approval_matrix: true };
  } catch { return refuse('approval-fault'); }
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
  process.stdout.write(`${JSON.stringify(proof.verified === false ? { ...proof, stage: refusedAt } : proof)}\n`);
}
