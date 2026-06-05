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
 *  any `:visible.first()` locator. On mobile the threads drawer doesn't
 *  auto-close after Archive, so leftover threads can also cover the prompt
 *  area on the next test's compose view — clearing thread_summaries lets
 *  newThread reliably land on an empty compose. Deliberately does NOT
 *  truncate `events`: that desyncs in-memory CC session state and stalls
 *  follow-on CC tests on their commands fetch. */
export function clearAllThreads(): void {
  psql([
    "TRUNCATE TABLE thread_summaries CASCADE",
    "TRUNCATE TABLE notifications CASCADE",
  ].join(';\n'));
}

/** Wipe the notification projection AND the source events. Used by specs that
 *  POST a notification and want the next `NotificationCreated` SSE to land on
 *  a clean bell badge instead of being shadowed by a backlog. The targeted
 *  event-type delete (not `TRUNCATE TABLE events`) leaves CC session state
 *  intact — `clearAllThreads` explicitly avoids touching `events` for the
 *  same reason. */
export function clearNotifications(): void {
  psql([
    "DELETE FROM notifications",
    "DELETE FROM events WHERE event_type IN ('NotificationCreated','NotificationRead','NotificationsAllRead')",
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

/** Direct `thread_summaries` projection insert for drawer-shape tests —
 *  bypasses the chat flow since the drawer only reads from the projection.
 *  `state='active'` is required: the column default is 'composing' which
 *  categorizeThreads filters out entirely. Default `archive_state='archived'`
 *  keeps every seeded thread in one section so families nest together. */
export function seedThreadRow({ id, title, parentId, totalChildren = 0, now }: {
  id: string;
  title: string;
  parentId?: string;
  totalChildren?: number;
  now: string;
}): string {
  const cols = ['thread_id', 'title', 'source', 'last_activity', 'message_count',
    'is_saved', 'has_response', 'status', 'archive_state', 'state',
    'is_coding_agent', 'active_children_count', 'total_children_count',
    'coding_agent_proposed', 'coding_agent_requires_restart',
    'coding_agent_is_external_repo',
    ...(parentId ? ['parent_thread_id'] : []),
  ].join(', ');
  const vals = [`'${id}'`, `'${title}'`, `'chat'`, `'${now}'`, '1',
    'false', 'true', `'idle'`, `'archived'`, `'active'`,
    'false', '0', String(totalChildren),
    'false', 'false', 'false',
    ...(parentId ? [`'${parentId}'`] : []),
  ].join(', ');
  return `INSERT INTO thread_summaries (${cols}) VALUES (${vals})`;
}

/** Create a CC thread with a pending change (git branch + DB rows). */
export function createCCThreadWithChange(titlePrefix: string, suffix: string, opts: {
  requiresRestart?: boolean;
} = {}): {
  threadId: string; changeId: string; branch: string; file: string;
} {
  const threadId = randomUUID();
  const changeId = randomUUID();
  const branch = `e2e-test/${suffix}`;
  const file = `e2e-${suffix}.txt`;
  const now = new Date().toISOString();
  const requiresRestart = opts.requiresRestart ?? false;

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
    // coding_agent_has_diff=true: the helper commits a real change on `branch`
    // (above), so the branch genuinely has a diff. The WaitingBanner Diff button
    // gates on this column (see WaitingBanner.getWaitingState `showDiff`); the
    // direct projection insert bypasses EventBus, so the CodingAgentIdled
    // {has_changes:true} event below never updates it — set it explicitly.
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_has_diff) VALUES ('${threadId}', '${titlePrefix} ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, true, ${requiresRestart}, false, true)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respEventId}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${idleEventId}', 'CodingAgentIdled', '{"has_changes":true,"is_external_repo":false,"requires_restart":${requiresRestart}}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id) VALUES ('${changeId}', '${requestId}', '${branch}', '${WORKSPACE}', '${titlePrefix} change ${suffix}', 1, ARRAY['${file}'], ${requiresRestart}, true, '${threadId}')`,
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

/** Like createCCThreadWithChange but stamps the thread as an *app coding-agent
 *  thread* (`coding_agent_kind='app'`, `coding_agent_folder=<ws>/data/apps/<id>`)
 *  and creates an app-shaped worktree+branch. Used by the WIP-preview / apply
 *  app-cc specs. Both the app folder and the workspace commit must already
 *  exist — pair with `createIframeAppFixture` + a one-time
 *  `git add data/apps/<id> && git commit`. */
export function createAppCCThreadWithChange(opts: {
  appId: string;
  titlePrefix: string;
  suffix: string;
  /** File extension for the seeded change file. Use `.html`/`.css`/`.js`/
   *  `manifest.json` etc. when the caller needs `AppUiRefreshRequested` to
   *  fire on Apply — `any_iframe_bundled_file_changed` only triggers for
   *  iframe-bundled extensions under the app folder. Default `.txt` is fine
   *  for specs that don't depend on the refresh. */
  fileExt?: string;
}): { threadId: string; changeId: string; branch: string; file: string } {
  const { appId, titlePrefix, suffix } = opts;
  const ext = opts.fileExt ?? '.txt';
  const threadId = randomUUID();
  const changeId = randomUUID();
  const branch = `claude-code/app/${appId}/${suffix}-${randomUUID().slice(0, 8)}`;
  const file = `data/apps/${appId}/e2e-${suffix}${ext}`;
  const now = new Date().toISOString();
  const folder = resolve(WORKSPACE, 'data/apps', appId);

  // Branch carrying the change. Don't bother with a real sparse-checkout
  // worktree here — apply against an in-place branch works the same and the
  // spec doesn't test worktree mechanics.
  git(['checkout', '-b', branch, 'main']);
  writeFileSync(resolve(WORKSPACE, file), `app cc test content ${suffix}`);
  git(['add', '.']);
  git(['commit', '-m', `e2e app cc test ${suffix}`]);
  git(['checkout', 'main']);

  const msgEventId = randomUUID();
  const respEventId = randomUUID();
  const idleEventId = randomUUID();
  const requestId = randomUUID();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_has_diff, coding_agent_kind, coding_agent_folder) VALUES ('${threadId}', '${titlePrefix} ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, true, false, false, true, 'app', '${folder}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respEventId}', 'SessionStarted', '{"session_id":"e2e-${suffix}","branch":"${branch}","coding_agent_kind":"app","coding_agent_folder":"${folder}","app_id":"${appId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${idleEventId}', 'CodingAgentIdled', '{"has_changes":true,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id) VALUES ('${changeId}', '${requestId}', '${branch}', '${WORKSPACE}', '${titlePrefix} app cc change ${suffix}', 1, ARRAY['${file}'], false, false, '${threadId}')`,
  ].join(';\n'));

  return { threadId, changeId, branch, file };
}
