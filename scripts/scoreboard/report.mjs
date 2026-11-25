import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { perfSamplesSection } from './perf-samples.mjs';
import { buildScoreboard, crateGraph, SCOREBOARD_EVIDENCE, suiteTimings, summarize } from './unit-costs.mjs';

export const PAGE_SIZE = 10_000;
const REPO_ROOT = fileURLToPath(new URL('../..', import.meta.url));
const USAGE = `Usage: scripts/scoreboard.sh [--days N] [--json]

  --days N   report changes that moved in the last N days (default 7)
  --json     print the report as JSON instead of text`;

const PARAM_ENV = {
  host: 'PGHOST',
  port: 'PGPORT',
  user: 'PGUSER',
  options: 'PGOPTIONS',
  sslmode: 'PGSSLMODE',
  sslrootcert: 'PGSSLROOTCERT',
  application_name: 'PGAPPNAME',
  connect_timeout: 'PGCONNECT_TIMEOUT',
};

export function pgEnv(url) {
  const u = new URL(url);
  if (!/^postgres(ql)?:$/.test(u.protocol)) throw new Error('LUCIDOS_DATABASE_URL is not a postgres URL');
  const env = {};
  if (u.hostname) env.PGHOST = decodeURIComponent(u.hostname.replace(/^\[|\]$/g, ''));
  if (u.port) env.PGPORT = u.port;
  if (u.username) env.PGUSER = decodeURIComponent(u.username);
  if (u.password) env.PGPASSWORD = decodeURIComponent(u.password);
  const database = decodeURIComponent(u.pathname.replace(/^\//, ''));
  if (database) env.PGDATABASE = database;
  for (const [key, value] of u.searchParams) {
    if (!PARAM_ENV[key]) throw new Error(`LUCIDOS_DATABASE_URL has a parameter the scoreboard cannot pass: ${key}`);
    env[PARAM_ENV[key]] = value;
  }
  env.PGOPTIONS = [env.PGOPTIONS, '-c default_transaction_read_only=on'].filter(Boolean).join(' ');
  return env;
}

export function pageSql(evidence, afterSeq, limit) {
  if (!Number.isSafeInteger(afterSeq) || !Number.isSafeInteger(limit)) throw new Error('page bounds must be integers');
  const types = Object.keys(evidence).sort();
  const fields = (list) => (list.length ? `jsonb_build_object(${list.map((f) => `${lit(f)}, p->${lit(f)}`).join(', ')})` : `'{}'::jsonb`);
  return `select coalesce(json_agg(r order by r.seq), '[]') from (
  select e.sequence seq, e.created at, e.event_type type,
         coalesce(e.thread_id::text, p->>'thread_id', p->'actor'->>'source_thread_id') thread,
         jsonb_strip_nulls(case e.event_type ${types.map((t) => `when ${lit(t)} then ${fields(evidence[t])}`).join(' ')} end) data
  from events e,
       lateral (select case when e.payload->>'type' = e.event_type and e.payload ? 'data'
                            then e.payload->'data' else e.payload end p) u
  where e.event_type in (${types.map(lit).join(', ')}) and e.sequence > ${afterSeq}
  order by e.sequence limit ${limit}) r`;
}

function lit(s) {
  return `'${String(s).replace(/'/g, "''")}'`;
}

export function fetchAll(page, limit = PAGE_SIZE) {
  const rows = [];
  let after = 0;
  for (;;) {
    const batch = page(after, limit);
    rows.push(...batch);
    if (batch.length < limit) return rows;
    after = batch.at(-1).seq;
  }
}

function psqlJson(sql, env) {
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([k]) => !k.startsWith('PG')));
  const run = spawnSync('psql', ['-X', '-q', '-At', '-v', 'ON_ERROR_STOP=1', '-f', '-'], {
    input: sql,
    env: { ...inherited, ...env },
    encoding: 'utf8',
    maxBuffer: 1 << 30,
  });
  if (run.error) throw run.error;
  if (run.status !== 0) throw new Error(`psql failed: ${run.stderr.trim()}`);
  return JSON.parse(run.stdout);
}

function windowSql(observedMs, days) {
  return `to_timestamp(${observedMs / 1000}) - make_interval(days => ${days})`;
}

function mainCheckSql(since) {
  return `with s as (select payload->'data'->>'check_id' id, min(created) t0 from events
           where event_type = 'MainCheckStarted' and created > ${since} group by 1),
     v as (select payload->'data'->>'check_id' id, event_type outcome, min(created) t1 from events
           where event_type in ('MainCheckPassed', 'MainCheckFailed') and created > ${since} group by 1, 2)
select coalesce(json_agg(json_build_object('outcome', v.outcome, 'seconds', extract(epoch from v.t1 - s.t0))), '[]')
from s join v using (id)`;
}

function laneBatchSql(since) {
  return `with b as (select aggregate_id id, min(created) t0 from events
           where event_type = 'LaneBatchStarted' and created > ${since} group by 1),
     e as (select aggregate_id id, event_type outcome, min(created) t1 from events
           where event_type in ('LaneBatchLanded', 'LaneBatchFailed') and created > ${since} group by 1, 2),
     h as (select b.id, count(distinct h.payload->>'head_sha') rounds,
                  min(h.created) filter (where h.payload->>'verdict' = 'pass') first_pass
           from b join events h on h.event_type = 'HardenRunCompleted'
                               and h.payload->>'branch' = 'lane/' || b.id and h.created > b.t0
           group by b.id)
select coalesce(json_agg(json_build_object('outcome', coalesce(e.outcome, 'open'), 'rounds', coalesce(h.rounds, 0),
                                           'pass_to_land_seconds', extract(epoch from e.t1 - h.first_pass))), '[]')
from b left join e using (id) left join h using (id)`;
}

export function groupSummaries(rows, key, value) {
  const groups = {};
  for (const row of rows) (groups[row[key]] ||= []).push(row[value]);
  return Object.fromEntries(Object.keys(groups).sort().map((k) => [k, summarize(groups[k])]));
}

export function githubBuilds(text, { observedMs, days }) {
  const since = observedMs - days * 86_400_000;
  const jobs = [];
  for (const line of text.split('\n')) {
    let record;
    try {
      record = JSON.parse(line);
    } catch {
      continue;
    }
    if (!(Date.parse(record?.pushed_at) >= since)) continue;
    for (const job of record.jobs || []) {
      if (job.name?.startsWith('build-') && job.conclusion === 'success') jobs.push({ name: job.name, seconds: job.run_secs });
    }
  }
  return groupSummaries(jobs, 'name', 'seconds');
}

function readCrateGraph() {
  const run = spawnSync('cargo', ['metadata', '--no-deps', '--format-version', '1', '--offline'], {
    cwd: REPO_ROOT,
    encoding: 'utf8',
    maxBuffer: 1 << 28,
  });
  return run.status === 0 ? crateGraph(JSON.parse(run.stdout)) : null;
}

export function parseArgs(argv) {
  const args = { days: 7, json: false, timingsLog: null, cargoSuites: [] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--days') args.days = argv[++i];
    else if (arg === '--json') args.json = true;
    else if (arg === '--timings-log') args.timingsLog = argv[++i];
    else if (arg === '--cargo-suites') args.cargoSuites = (argv[++i] || '').split(/\s+/).filter(Boolean);
    else if (arg === '-h' || arg === '--help') args.help = true;
    else throw new Error(`unknown argument '${arg}'\n${USAGE}`);
  }
  if (!/^\d+$/.test(String(args.days))) throw new Error(`--days takes a whole number of days, got '${args.days}'`);
  args.days = Number(args.days);
  return args;
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    console.log(USAGE);
    return;
  }
  const url = process.env.LUCIDOS_DATABASE_URL;
  if (!url) throw new Error('LUCIDOS_DATABASE_URL is not set. Run this from a Lucidos agent shell.');
  const env = pgEnv(url);
  const observedMs = Date.now();
  const observedAt = new Date(observedMs).toISOString();
  const events = fetchAll((after, limit) => psqlJson(pageSql(SCOREBOARD_EVIDENCE, after, limit), env));
  const since = windowSql(observedMs, args.days);
  const crates = readCrateGraph();
  const report = {
    ...buildScoreboard(events, { observedAt, days: args.days, crates }),
    crate_graph: crates ? 'cargo metadata' : 'unavailable',
    harden_suites: suiteTimings(events, { observedAt, days: args.days, cargoSuites: args.cargoSuites }),
    main_check_seconds: groupSummaries(psqlJson(mainCheckSql(since), env), 'outcome', 'seconds'),
    lane_batches: laneBatches(psqlJson(laneBatchSql(since), env)),
    github_build_seconds: githubSection(args.timingsLog, { observedMs, days: args.days }),
    perf_samples: perfSamplesSection({ days: args.days, observedMs }),
  };
  console.log(args.json ? JSON.stringify(report, null, 2) : formatText(report));
}

function laneBatches(rows) {
  const out = {};
  for (const outcome of [...new Set(rows.map((r) => r.outcome))].sort()) {
    const batch = rows.filter((r) => r.outcome === outcome);
    out[outcome] = {
      batches: batch.length,
      rounds: summarize(batch.map((r) => r.rounds)),
      pass_to_land_seconds: summarize(batch.map((r) => r.pass_to_land_seconds)),
    };
  }
  return out;
}

function githubSection(path, window) {
  if (!path) return { unavailable: 'no timings log path' };
  try {
    return githubBuilds(readFileSync(path, 'utf8'), window);
  } catch (error) {
    if (error.code === 'ENOENT') return { unavailable: `no timings log at ${path}` };
    throw error;
  }
}


const minutes = (ms) => (ms / 60_000).toFixed(1);
const fromSeconds = (s) => (s / 60).toFixed(1);
const whole = (ms) => String(Math.round(ms));
const count = (n) => String(Number(n.toFixed(1)));
const TEXT_ROWS = 15;

export function cell(summary, unit = minutes) {
  if (!summary) return '-';
  if (summary.tooFew) return `too few (n=${summary.n})`;
  return `n=${summary.n} p50 ${unit(summary.p50)} p90 ${unit(summary.p90)}`;
}

function table(headers, rows) {
  const widths = headers.map((h, i) => Math.max(h.length, ...rows.map((r) => String(r[i]).length)));
  const line = (r) => r.map((v, i) => String(v).padEnd(widths[i])).join('  ').trimEnd();
  return [line(headers), ...rows.map(line)].join('\n');
}

function counts(map) {
  const entries = Object.entries(map).sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  if (!entries.length) return 'none';
  const shown = entries.slice(0, TEXT_ROWS).map(([k, n]) => `${k} ${n}`);
  const rest = entries.length - shown.length;
  return [...shown, ...(rest > 0 ? [`and ${rest} more`] : [])].join(', ');
}

export function formatText(r) {
  const out = [];
  const section = (title) => out.push('', `== ${title}`);
  section(`Delivery: changes that moved in the last ${r.days} days (observed ${r.observed_at})`);
  out.push(`changes ${r.coverage.changes}, lead time known for ${r.coverage.lead_time_known}; unknown: ${counts(r.coverage.lead_time_unknown)}`);
  out.push(`aggregates over fewer than ${r.too_few_below} changes show "too few"; times in minutes`);
  out.push(table(['measure', 'value'], [['lead time', cell(r.lead_time)], ['waits on you', cell(r.waits)]]));

  section('Stages (minutes per closed visit)');
  out.push(table(['stage', 'visits', 'open', 'unknown'], Object.entries(r.stages).map(([s, v]) => [s, cell(v.visits), v.open, v.unknown])));

  const recent = [...r.changes].sort((a, b) => (b.live_at ?? b.landed_at ?? '').localeCompare(a.live_at ?? a.landed_at ?? ''));
  section(`Changes, latest landed first (${Math.min(TEXT_ROWS, recent.length)} of ${recent.length})`);
  const short = (id) => (id ? String(id).slice(0, 8) : '-');
  const opt = (ms) => (ms == null ? '-' : minutes(ms));
  out.push(
    table(
      ['thread', 'change', 'stage', 'lead', 'waits', 'hardening', 'rounds', 'unknown'],
      recent
        .slice(0, TEXT_ROWS)
        .map((c) => [short(c.thread), short(c.change_id), c.stage ?? '-', opt(c.lead_time_ms), opt(c.wait_ms), opt(c.hardening_ms), c.rounds, [c.unknown, ...c.unknown_stages].filter(Boolean).join(' ') || '-']),
    ),
  );

  section(`Classes (lead time in minutes, harden rounds; crate graph: ${r.crate_graph})`);
  for (const [attribute, values] of Object.entries(r.classes)) {
    out.push(attribute);
    out.push(table(['  value', 'changes', 'lead time', 'rounds'], Object.entries(values).map(([v, s]) => [`  ${v}`, s.changes, cell(s.lead_time), cell(s.rounds, count)])));
  }

  const u = r.unit_costs;
  section('Unit costs');
  out.push(`harden rounds per landed change: ${cell(u.rounds_per_change, count)} (landed with no round on record: ${u.changes_without_rounds})`);
  out.push(`failed phases: ${counts(u.failed_phases)}`);
  out.push(`minutes per round: ${cell(u.round_seconds, fromSeconds)} (rounds with no start on record: ${u.round_seconds.without_start})`);
  out.push(`engine build after Apply, minutes: ${Object.entries(u.engine_builds).map(([o, s]) => `${o} ${cell(s)}`).join('; ') || 'none on record'}`);
  out.push(`frontend refresh after Apply, minutes: ${cell(u.frontend_refresh)}`);
  const slots = Object.entries(u.build_slot).filter(([, s]) => s.wait_ms.n > 0);
  const unrecorded = Object.values(u.build_slot).reduce((n, s) => n + s.unrecorded, 0);
  out.push(`build slot by label, minutes (releases with no wait or hold on record: ${unrecorded}):`);
  slots.sort((a, b) => b[1].hold_ms.n - a[1].hold_ms.n);
  if (slots.length) out.push(table(['  label', 'wait', 'hold'], slots.slice(0, TEXT_ROWS).map(([l, s]) => [`  ${l}`, cell(s.wait_ms), cell(s.hold_ms)])));
  else out.push('  none on record');

  section('Rework after landing');
  out.push(`reverted ${r.rework.reverted}, apply failed ${r.rework.apply_failed}, main check failed ${r.rework.main_check_failed}`);

  section('Harden round span by shape, full rounds (minutes)');
  out.push(table(['shape', 'span', 'failing', 'first fail to verdict'], Object.entries(r.harden_suites.shapes).map(([s, v]) => [s, cell(v.span_s, fromSeconds), v.failing, cell(v.first_fail_to_verdict_s, fromSeconds)])));

  section('Per suite, full rounds (minutes)');
  out.push(table(['suite', 'run', 'wait', 'failed'], Object.entries(r.harden_suites.suites).map(([s, v]) => [s, cell(v.run_s, fromSeconds), cell(v.wait_s, fromSeconds), v.failed])));

  section('Main check: start to verdict (minutes)');
  out.push(table(['outcome', 'time'], Object.entries(r.main_check_seconds).map(([o, s]) => [o, cell(s, fromSeconds)])));

  section('Lane batches: harden rounds, and first passing round to landing (minutes)');
  out.push(table(['outcome', 'batches', 'rounds', 'pass to land'], Object.entries(r.lane_batches).map(([o, b]) => [o, b.batches, cell(b.rounds, count), cell(b.pass_to_land_seconds, fromSeconds)])));

  section('GitHub build jobs on this host (minutes)');
  if (r.github_build_seconds.unavailable) out.push(r.github_build_seconds.unavailable);
  else out.push(table(['job', 'time'], Object.entries(r.github_build_seconds).map(([j, s]) => [j, cell(s, fromSeconds)])));

  const p = r.perf_samples;
  section('App speed samples from the diagnostic log (milliseconds per sample kind and device class)');
  if (p.unavailable) out.push(`unavailable: ${p.unavailable}`);
  else if (p.rollup.length) out.push(table(['kind', 'device', 'samples'], p.rollup.map((s) => [s.kind, s.device, cell(s, whole)])));
  for (const note of p.notes) out.push(`note: ${note}`);
  return out.join('\n').trimStart();
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  try {
    main();
  } catch (error) {
    console.error(`scoreboard: ${error.message}`);
    process.exit(2);
  }
}
