import { execSync } from 'child_process';
import { existsSync, mkdirSync, writeFileSync, rmSync, realpathSync, readFileSync } from 'fs';
import { homedir } from 'os';
import { join, resolve } from 'path';
import { randomUUID } from 'crypto';
import { E2E_WORKSPACE } from './address';

export const WORKSPACE = E2E_WORKSPACE;

export const WORKSPACE_CANONICAL = (() => {
  try { return realpathSync(WORKSPACE); } catch { return WORKSPACE; }
})();

export function git(args: string[]): string {
  const quoted = args.map(a => a.includes(' ') ? `"${a}"` : a).join(' ');
  return execSync(`git ${quoted}`, { cwd: WORKSPACE, encoding: 'utf-8' });
}

let cachedDbPort: string | null = null;
export function getDbPort(): string {
  if (cachedDbPort) return cachedDbPort;
  const external = process.env.LUCIDOS_EXTERNAL_PG_PORT;
  if (external) return (cachedDbPort = external);
  const container = process.env.LUCIDOS_SHARED_PG_CONTAINER ?? 'lucidos-pg-shared';
  const portLine = execSync(`docker port ${container} 5432`, { encoding: 'utf-8' }).trim();
  cachedDbPort = portLine.split(':').pop()!;
  return cachedDbPort;
}

export function getDbName(): string {
  const basename = WORKSPACE.split(/[\\/]/).filter(Boolean).pop() ?? 'workspace';
  const slug = basename.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'workspace';
  return `lucidos_${slug}`;
}

export function clearAllThreads(): void {
  psql([
    "TRUNCATE TABLE thread_summaries CASCADE",
    "TRUNCATE TABLE notifications CASCADE",
  ].join(';\n'));
}

export function homeThreadIdInDb(): string | null {
  return psql('SELECT thread_id FROM thread_summaries WHERE is_home') || null;
}

export function ensureHomeThread(): string {
  const existing = homeThreadIdInDb();
  if (existing) return existing;
  return psql(
    "INSERT INTO thread_summaries (thread_id, title, initiator, source, created_at, last_activity, " +
    "message_count, state, status, is_home) VALUES (gen_random_uuid(), 'Home', 'user', 'chat', " +
    "NOW(), NOW(), 0, 'active', 'idle', TRUE) RETURNING thread_id",
  ).split('\n')[0].trim();
}

export function clearNotifications(): void {
  psql([
    "DELETE FROM notifications",
    "DELETE FROM events WHERE event_type IN ('NotificationCreated','NotificationRead','NotificationsAllRead')",
  ].join(';\n'));
}

export function resetWelcomePreference(): void {
  psql([
    "DELETE FROM preferences WHERE key = 'welcome_suggestions_dismissed'",
    "DELETE FROM events WHERE event_type = 'PreferencesChanged' AND payload->>'key' = 'welcome_suggestions_dismissed'",
  ].join(';\n'));
}

function sharedPgPassword(): string {
  const entry = 'pg-shared-password';
  const moved = process.env.LUCIDOS_PRIVATE_DIR;
  const home = join(homedir(), '.lucidos');
  const older = join(home, entry);
  const path = moved ? join(moved, entry) : existsSync(older) ? older : join(home, 'private', entry);
  return readFileSync(path, 'utf-8').trim();
}

export function psql(sql: string): string {
  const dbPort = getDbPort();
  return execSync(
    `psql "postgres://lucidos@localhost:${dbPort}/${getDbName()}" -t`,
    { encoding: 'utf-8', input: sql, env: { ...process.env, PGPASSWORD: sharedPgPassword() } },
  ).trim();
}

export function seedThreadRow({
  id, title, parentId, totalChildren = 0, now, source = 'chat', status = 'idle', archiveState = 'archived',
}: {
  id: string;
  title: string;
  parentId?: string;
  totalChildren?: number;
  now: string;
  source?: 'chat' | 'trigger';
  status?: 'idle' | 'failed';
  archiveState?: 'archived' | 'inbox';
}): string {
  const cols = ['thread_id', 'title', 'source', 'created_at', 'last_activity', 'message_count',
    'is_saved', 'has_response', 'status', 'archive_state', 'state',
    'is_coding_agent', 'active_children_count', 'total_children_count',
    'coding_agent_change_state',
    'coding_agent_is_external_repo',
    ...(parentId ? ['parent_thread_id'] : []),
  ].join(', ');
  const vals = [`'${id}'`, `'${title}'`, `'${source}'`, `'${now}'`, `'${now}'`, '1',
    'false', 'true', `'${status}'`, `'${archiveState}'`, `'active'`,
    'false', '0', String(totalChildren),
    `'none'`, 'false',
    ...(parentId ? [`'${parentId}'`] : []),
  ].join(', ');
  return `INSERT INTO thread_summaries (${cols}) VALUES (${vals})`;
}

export function seedStepHeavyThread({ turns, stepsPerTurn, title }: {
  turns: number;
  stepsPerTurn: number;
  title: string;
}): { threadId: string; messageIds: string[] } {
  const threadId = randomUUID();
  const now = new Date().toISOString();
  const messageIds: string[] = [];
  const row = (id: string, type: string, payload: string) =>
    `('${id}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const rows: string[] = [];
  for (let t = 0; t < turns; t++) {
    const messageId = randomUUID();
    messageIds.push(messageId);
    rows.push(row(messageId, 'MessageReceived', `{"text":"turn ${t}","mode":"human","channel":"claude_code"}`));
    for (let s = 0; s < stepsPerTurn; s++) {
      const useId = `e2e-${threadId}-${t}-${s}`;
      rows.push(row(randomUUID(), 'CodingAgentToolCalled',
        `{"name":"Bash","args":{"command":"echo ${t}.${s}"},"description":"Run echo ${t}.${s}",` +
        `"channel":"claude_code","tool_use_id":"${useId}","coding_agent":"claude-code",` +
        `"request_event_id":"${messageId}"}`));
      rows.push(row(randomUUID(), 'CodingAgentToolResult',
        `{"name":"","result":"${t}.${s} done","channel":"claude_code","tool_use_id":"${useId}",` +
        `"coding_agent":"claude-code","request_event_id":"${messageId}"}`));
    }
    rows.push(row(randomUUID(), 'ResponseGenerated',
      `{"text":"Finished turn ${t}.","images":[],"request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', '${title}', 'claude_code', '${now}', ${turns}, false, true, 'idle', 'archived', 'active', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return { threadId, messageIds };
}

export function seedChatThread({ turns, needles, title }: {
  turns: number;
  needles: number[];
  title: string;
}): string {
  const threadId = randomUUID();
  const base = Date.now() - turns * 10_000;
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row = (id: string, type: string, payload: string) =>
    `('${id}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;
  for (let t = 0; t < turns; t++) {
    const messageId = randomUUID();
    const reply = needles.includes(t) ? `Answer ${t} mentions the zebra.` : `Answer ${t}.`;
    const ref = `"request_event_id":"${messageId}"`;
    rows.push(
      row(messageId, 'MessageReceived', `{"text":"Question ${t}","mode":"human","channel":"chat"}`),
      row(randomUUID(), 'TextStreamed', `{"text":"${reply}",${ref}}`),
      row(randomUUID(), 'ResponseGenerated', `{"text":"${reply}","images":[],${ref}}`),
    );
  }
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', '${title}', 'chat', '${new Date().toISOString()}', ${turns}, false, true, 'idle', 'archived', 'active', false, 0, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n${rows.join(',\n')}`,
  ].join(';\n'));
  return threadId;
}

export function seedThreadOfCards({ word, title }: { word: string; title: string }): string {
  const threadId = randomUUID();
  const base = Date.now() - 60_000;
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const row = (type: string, payload: string, id = randomUUID()) =>
    `('${id}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;
  const messageId = randomUUID();
  const ref = `"request_event_id":"${messageId}"`;
  const rows = [
    row('MessageReceived', '{"text":"Plan the zoo","mode":"human","channel":"chat"}', messageId),
    row('ToolCalled', `{"name":"read_file","args":{"path":"notes/${word}.md"},${ref}}`),
    row('ToolResult', `{"name":"read_file","result":"ok",${ref}}`),
    row('TextStreamed', `{"text":"Read the notes.",${ref}}`),
    row('UserQuestionAsked', `{"tool_use_id":"q1","question":"Which plan?","multi_select":false,`
      + `"options":[{"id":"a","label":"Approve","description":"Approves the ${word} plan."}],${ref}}`),
    row('UserQuestionAnswered', '{"tool_use_id":"q1","answer":{"kind":"Selected","option_id":"a"}}'),
  ];
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) `
      + `VALUES ('${threadId}', '${title}', 'chat', '${new Date().toISOString()}', 1, false, true, 'idle', 'archived', 'active', false, 0, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n${rows.join(',\n')}`,
  ].join(';\n'));
  return threadId;
}

export function seedThreadOfCounters({ turns, stepsPerTurn, title }: {
  turns: number;
  stepsPerTurn: number;
  title: string;
}): string {
  const threadId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row = (type: string, payload: string) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;

  for (let t = 0; t < turns; t++) {
    const messageId = randomUUID();
    rows.push(`('${messageId}', 'MessageReceived', '{"text":"turn ${t}","mode":"human","channel":"chat"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
    for (let i = 0; i < stepsPerTurn; i++) {
      const ref = `"request_event_id":"${messageId}"`;
      rows.push(
        row('ThoughtStreamed', `{"text":"","context_tokens":${40_000 + i * 1000},"context_messages":${i + 2},${ref}}`),
        row('ToolCalled', `{"name":"read_file","args":{"path":"notes/${t}-${i}.md"},${ref}}`),
        row('ToolResult', `{"name":"read_file","result":"ok",${ref}}`),
      );
    }
    rows.push(row('ResponseGenerated', `{"text":"Done ${t}.","images":[],"request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', '${title}', 'chat', '${new Date(base).toISOString()}', ${turns}, false, true, 'idle', 'archived', 'active', false, 0, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

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
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', '${titlePrefix} ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, 'proposed', ${requiresRestart}, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respEventId}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${idleEventId}', 'CodingAgentIdled', '{"has_changes":true,"is_external_repo":false,"requires_restart":${requiresRestart}}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id) VALUES ('${changeId}', '${requestId}', '${branch}', '${WORKSPACE}', '${titlePrefix} change ${suffix}', 1, ARRAY['${file}'], ${requiresRestart}, true, '${threadId}')`,
    `INSERT INTO planned_branches (repo_root, branch_name, state, head_sha) VALUES ('${WORKSPACE_CANONICAL}', '${branch}', 'acknowledged_simple', 'seeded') ON CONFLICT (repo_root, branch_name) DO NOTHING`,
  ].join(';\n'));

  return { threadId, changeId, branch, file };
}

export function cleanupCCThread(threadId: string, changeId?: string, branch?: string, file?: string): void {
  if (file) try { execSync(`rm -f "${resolve(WORKSPACE, file)}"`, { encoding: 'utf-8' }); } catch {  }
  if (branch) try { git(['branch', '-D', branch]); } catch {  }
  try { psql([
    ...(changeId ? [`DELETE FROM changes WHERE id = '${changeId}'`] : []),
    ...(branch ? [`DELETE FROM planned_branches WHERE branch_name = '${branch}'`] : []),
    `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
    `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
  ].join(';\n')); } catch {  }
}

export function cleanupFileFromMain(file: string, suffix: string): void {
  try {
    execSync(`rm -f "${resolve(WORKSPACE, file)}"`, { encoding: 'utf-8' });
    git(['add', '.']);
    git(['commit', '-m', `chore: clean up e2e test file ${suffix}`]);
  } catch {  }
}

export function appPath(id: string): string {
  return `/app/${id}/`;
}

export function createIframeAppFixture(id: string, files: {
  html: string;
  js: string;
  manifest?: Record<string, unknown>;
  extra?: Record<string, string | Uint8Array>;
}): { dir: string; cleanup: () => void } {
  const dir = resolve(WORKSPACE, 'data/apps', id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(resolve(dir, 'index.html'), files.html);
  writeFileSync(resolve(dir, 'script.js'), files.js);
  writeFileSync(
    resolve(dir, 'manifest.json'),
    JSON.stringify(files.manifest ?? { id, name: id, description: 'e2e fixture' }),
  );
  for (const [name, body] of Object.entries(files.extra ?? {})) {
    writeFileSync(resolve(dir, name), body);
  }
  return {
    dir,
    cleanup: () => rmSync(dir, { recursive: true, force: true }),
  };
}

export function createAppCCThreadWithChange(opts: {
  appId: string;
  titlePrefix: string;
  suffix: string;
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
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo, coding_agent_kind, coding_agent_folder) VALUES ('${threadId}', '${titlePrefix} ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, 'proposed', false, 'app', '${folder}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${respEventId}', 'SessionStarted', '{"session_id":"e2e-${suffix}","branch":"${branch}","coding_agent_kind":"app","coding_agent_folder":"${folder}","app_id":"${appId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${idleEventId}', 'CodingAgentIdled', '{"has_changes":true,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO changes (id, request_id, branch_name, repo_root, description, file_count, files, requires_restart, hardened, thread_id) VALUES ('${changeId}', '${requestId}', '${branch}', '${WORKSPACE}', '${titlePrefix} app cc change ${suffix}', 1, ARRAY['${file}'], false, false, '${threadId}')`,
  ].join(';\n'));

  return { threadId, changeId, branch, file };
}
