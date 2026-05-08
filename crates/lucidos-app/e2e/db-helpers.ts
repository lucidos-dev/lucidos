import { execSync } from 'child_process';
import { mkdirSync, writeFileSync, rmSync } from 'fs';
import { resolve } from 'path';
import { randomUUID } from 'crypto';

export const WORKSPACE = resolve(process.env.E2E_WORKSPACE ?? `${process.env.HOME}/workspaces/e2e-test`);

export function git(args: string[]): string {
  const quoted = args.map(a => a.includes(' ') ? `"${a}"` : a).join(' ');
  return execSync(`git ${quoted}`, { cwd: WORKSPACE, encoding: 'utf-8' });
}

let cachedDbPort: string | null = null;
export function getDbPort(): string {
  if (cachedDbPort) return cachedDbPort;
  const cksum = execSync(`printf '%s' '${WORKSPACE}' | cksum | cut -d' ' -f1`, { encoding: 'utf-8' }).trim();
  const container = `lucidos-pg-${cksum}`;
  const portLine = execSync(`docker port ${container} 5432`, { encoding: 'utf-8' }).trim();
  cachedDbPort = portLine.split(':').pop()!;
  return cachedDbPort;
}

/** Wipe drawer state between tests in the same Playwright project. The DB
 *  resets only between projects, so survivors push the test's own row past
 *  any `:visible.first()` locator. Deliberately does NOT truncate `events`:
 *  that desyncs in-memory CC session state and stalls follow-on CC tests on
 *  their commands fetch. */
export function clearAllThreads(): void {
  psql([
    "TRUNCATE TABLE thread_summaries CASCADE",
    "TRUNCATE TABLE notifications CASCADE",
  ].join(';\n'));
}

/** Run SQL via stdin to avoid shell escaping issues with JSON payloads. */
export function psql(sql: string): string {
  const dbPort = getDbPort();
  return execSync(
    `psql "postgres://lucidos:lucidos@localhost:${dbPort}/lucidos" -t`,
    { encoding: 'utf-8', input: sql },
  ).trim();
}

/** Create a CC thread with a pending change (git branch + DB rows). */
export function createCCThreadWithChange(titlePrefix: string, suffix: string): {
  threadId: string; changeId: string; branch: string; file: string;
} {
  const threadId = randomUUID();
  const changeId = randomUUID();
  const branch = `e2e-test/${suffix}`;
  const file = `e2e-${suffix}.txt`;
  const now = new Date().toISOString();

  git(['checkout', '-b', branch, 'main']);
  writeFileSync(resolve(WORKSPACE, file), `test content ${suffix}`);
  git(['add', '.']);
  git(['commit', '-m', `e2e test ${suffix}`]);
  git(['checkout', 'main']);

  const msgEventId = randomUUID();
  const respEventId = randomUUID();
  const idleEventId = randomUUID();
  const requestId = randomUUID();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', '${titlePrefix} ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, true, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respEventId}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${idleEventId}', 'CodingAgentIdled', '{"has_changes":true,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id) VALUES ('${changeId}', '${requestId}', '${branch}', '${WORKSPACE}', '${titlePrefix} change ${suffix}', 1, ARRAY['${file}'], false, true, '${threadId}')`,
  ].join(';\n'));

  return { threadId, changeId, branch, file };
}

/** Clean up a CC thread's DB rows, git branch, and file. */
export function cleanupCCThread(threadId: string, changeId?: string, branch?: string, file?: string): void {
  if (file) try { execSync(`rm -f "${resolve(WORKSPACE, file)}"`, { encoding: 'utf-8' }); } catch { /* */ }
  if (branch) try { git(['branch', '-D', branch]); } catch { /* */ }
  try { psql([
    ...(changeId ? [`DELETE FROM changes WHERE id = '${changeId}'`] : []),
    `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
    `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
  ].join(';\n')); } catch { /* */ }
}

/** Remove an applied test file from main. */
export function cleanupFileFromMain(file: string, suffix: string): void {
  try {
    execSync(`rm -f "${resolve(WORKSPACE, file)}"`, { encoding: 'utf-8' });
    git(['add', '.']);
    git(['commit', '-m', `chore: clean up e2e test file ${suffix}`]);
  } catch { /* */ }
}

/** Engine-served URL prefix for an app's UI bundle. Single source of truth for specs. */
export function appPath(id: string): string {
  return `/app/${id}/`;
}

/**
 * Create an iframe app fixture under `data/apps/<id>/` and return a teardown
 * helper. Use from a Playwright spec's beforeAll/afterAll to test SDK
 * features that have to run inside an `appPath(id)` iframe.
 */
export function createIframeAppFixture(id: string, files: {
  html: string;
  js: string;
  manifest?: Record<string, unknown>;
}): { dir: string; cleanup: () => void } {
  const dir = resolve(WORKSPACE, 'data/apps', id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(resolve(dir, 'index.html'), files.html);
  writeFileSync(resolve(dir, 'script.js'), files.js);
  writeFileSync(
    resolve(dir, 'manifest.json'),
    JSON.stringify(files.manifest ?? { id, name: id, description: 'e2e fixture' }),
  );
  return {
    dir,
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}
