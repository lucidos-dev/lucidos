import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { cell, fetchAll, formatText, githubBuilds, pageSql, parseArgs, pgEnv } from './report.mjs';
import { buildScoreboard, SCOREBOARD_EVIDENCE, suiteTimings } from './unit-costs.mjs';

const fixture = JSON.parse(readFileSync(new URL('./fixtures/two-changes.json', import.meta.url), 'utf8'));

function pages(rows) {
  const calls = [];
  const page = (after, limit) => {
    calls.push(after);
    return rows.filter((r) => r.seq > after).slice(0, limit);
  };
  return { page, calls };
}

test('the fetch reads every page, not one bounded slice', () => {
  const { page, calls } = pages(fixture.events);
  const rows = fetchAll(page, 5);
  assert.equal(rows.length, fixture.events.length);
  assert.deepEqual(calls, [0, 5, 10]);
  const options = { observedAt: fixture.observed_at, days: 7 };
  assert.deepEqual(buildScoreboard(rows, options), buildScoreboard(fixture.events, options));
});

test('a page that exactly fills the limit asks once more', () => {
  const { page, calls } = pages(fixture.events.slice(0, 10));
  assert.equal(fetchAll(page, 5).length, 10);
  assert.deepEqual(calls, [0, 5, 10]);
});

test('the page query selects the declared evidence after a sequence', () => {
  const sql = pageSql(SCOREBOARD_EVIDENCE, 42, 100);
  assert.match(sql, /e\.sequence > 42/);
  assert.match(sql, /limit 100/);
  for (const type of Object.keys(SCOREBOARD_EVIDENCE)) assert.ok(sql.includes(`'${type}'`), type);
  assert.match(sql, /'HardeningStarted' then jsonb_build_object\('head_sha', p->'head_sha', 'iteration', p->'iteration', 'tier', p->'tier'\)/);
  assert.throws(() => pageSql(SCOREBOARD_EVIDENCE, '1; drop table events', 100), /integers/);
});

test('the database URL becomes PG variables, read-only, and never an argument', () => {
  const env = pgEnv('postgresql://agent:s%40cret@127.0.0.1:5435/example_db?options=-c%20statement_timeout%3D0');
  assert.deepEqual(env, {
    PGHOST: '127.0.0.1',
    PGPORT: '5435',
    PGUSER: 'agent',
    PGPASSWORD: 's@cret',
    PGDATABASE: 'example_db',
    PGOPTIONS: '-c statement_timeout=0 -c default_transaction_read_only=on',
  });
  assert.equal(pgEnv('postgres://localhost/db').PGOPTIONS, '-c default_transaction_read_only=on');
  assert.throws(() => pgEnv('postgresql://localhost/db?target_session_attrs=any'), /target_session_attrs/);
  assert.throws(() => pgEnv('mysql://localhost/db'), /not a postgres URL/);
});

test('arguments: days, json, and nothing unknown', () => {
  assert.deepEqual(parseArgs(['--days', '30', '--json']), { days: 30, json: true, timingsLog: null, cargoSuites: [] });
  assert.deepEqual(parseArgs(['--cargo-suites', 'crates lint']).cargoSuites, ['crates', 'lint']);
  assert.throws(() => parseArgs(['--days', 'week']), /whole number/);
  assert.throws(() => parseArgs(['--since']), /unknown argument/);
});

test('the text report shows the count behind every aggregate', () => {
  const report = {
    ...buildScoreboard(fixture.events, { observedAt: fixture.observed_at, days: 7 }),
    crate_graph: 'unavailable',
    harden_suites: suiteTimings(fixture.events, { observedAt: fixture.observed_at, days: 7, cargoSuites: [] }),
    main_check_seconds: {},
    lane_batches: {},
    github_build_seconds: { unavailable: 'no timings log path' },
    perf_samples: { unavailable: 'the lucidos command is not on PATH', notes: [] },
  };
  const text = formatText(report);
  assert.match(text, /App speed samples[^\n]*\nunavailable: the lucidos command is not on PATH/);
  assert.match(text, /lead time\s+too few \(n=2\)/);
  assert.match(text, /changes 2, lead time known for 2/);
  assert.match(text, /reverted 1, apply failed 0, main check failed 0/);
  assert.doesNotMatch(text, /p50 \S+ p90/, 'two changes never yield a median');
  assert.equal(cell({ n: 6, p50: 120_000, p90: 300_000 }), 'n=6 p50 2.0 p90 5.0');
});

test('GitHub build jobs read this host timings log within the window', () => {
  const lines = [
    JSON.stringify({ pushed_at: '2026-11-02T10:00:00Z', jobs: [{ name: 'build-engine', conclusion: 'success', run_secs: 600 }, { name: 'test', conclusion: 'success', run_secs: 5 }] }),
    JSON.stringify({ pushed_at: '2026-10-01T10:00:00Z', jobs: [{ name: 'build-engine', conclusion: 'success', run_secs: 900 }] }),
    'not json',
  ].join('\n');
  assert.deepEqual(githubBuilds(lines, { observedMs: Date.parse('2026-11-02T12:00:00Z'), days: 7 }), {
    'build-engine': { n: 1, tooFew: true },
  });
});

test('lab medians and counts are displayed per host and run', () => {
  const report = {
    ...buildScoreboard(fixture.events, { observedAt: fixture.observed_at, days: 7 }),
    harden_suites: suiteTimings(fixture.events, { observedAt: fixture.observed_at, days: 7, cargoSuites: [] }),
    main_check_seconds: {},
    lane_batches: {},
    github_build_seconds: { unavailable: 'no timings log' },
    perf_samples: { unavailable: 'no diagnostic log', notes: [] },
    lab_runs: [
      { kind: 'build', at: '2026-11-22', status: 'completed', host: {
        fingerprint: 'first-host', os: 'linux', architecture: 'arm64', cores: 10, memory_bytes: 48 * 1024 ** 3, toolchain: 'rustc 1',
      }, measurements: { build: { median: 2, samples: [1, 2, 3], unit: 'seconds' } } },
      { kind: 'app', at: '2026-11-22', status: 'deferred', host: {
        fingerprint: 'second-host', os: 'darwin', architecture: 'arm64', cores: 8, memory_bytes: 32 * 1024 ** 3, toolchain: 'v24',
      }, next_step: 'setup' },
    ],
  };
  const text = formatText(report);
  assert.match(text, /first-host.*linux arm64.*10 cores.*48 GiB.*rustc 1.*build.*2 seconds.*3/);
  assert.match(text, /second-host.*darwin arm64.*8 cores.*32 GiB.*v24.*setup.*deferred/);
});
