// Unit costs, classes and aggregates. Run: node --test 'scripts/scoreboard/*.test.mjs'
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { buildScoreboard, crateGraph, summarize, suiteTimings, TOO_FEW_BELOW } from './unit-costs.mjs';

const MIN = 60_000;

function load(name) {
  return JSON.parse(readFileSync(new URL(`./fixtures/${name}.json`, import.meta.url), 'utf8'));
}

// Two workspace crates: the app depends on the core, so the core is deeper.
const METADATA = {
  workspace_root: '/work/example-repo',
  packages: [
    { name: 'example-core', manifest_path: '/work/example-repo/crates/example-core/Cargo.toml', dependencies: [{ name: 'serde' }] },
    {
      name: 'example-app',
      manifest_path: '/work/example-repo/crates/example-app/Cargo.toml',
      dependencies: [{ name: 'example-core', path: '/work/example-repo/crates/example-core' }],
    },
  ],
};

test(`an aggregate over fewer than ${TOO_FEW_BELOW} values shows too few, with its count`, () => {
  assert.deepEqual(summarize([3]), { n: 1, tooFew: true });
  assert.deepEqual(summarize([]), { n: 0, tooFew: true });
  assert.deepEqual(summarize([1, 2, 3, 4, 5]), { n: 5, p50: 3, p90: 4.6 });
});

test('a class with one change shows too few, never a median', () => {
  const { events, observed_at } = load('worked-example');
  const board = buildScoreboard(events, { observedAt: observed_at, days: 7, crates: crateGraph(METADATA) });
  assert.deepEqual(board.classes.kind.feat, {
    changes: 1,
    lead_time: { n: 1, tooFew: true },
    rounds: { n: 1, tooFew: true },
  });
  assert.deepEqual(board.lead_time, { n: 1, tooFew: true });
  assert.deepEqual(board.coverage, { changes: 1, lead_time_known: 1, lead_time_unknown: {} });
  const [row] = board.changes;
  assert.equal(row.lead_time_ms, 100 * MIN);
  assert.equal(row.hardening_ms, 45 * MIN);
  assert.equal(row.rounds, 1);
});

test('the seven class attributes come from the change evidence', () => {
  const { events, observed_at } = load('worked-example');
  const extra = [
    { seq: 0.1, at: '2026-11-02T08:59:00Z', type: 'MessageReceived', thread: 'thread-a', data: { mode: 'human' } },
    { seq: 0.2, at: '2026-11-02T08:59:00Z', type: 'CodingAgentSettingsChanged', thread: 'thread-a', data: { coding_agent: 'claude-code', model: 'example-model', reasoning_effort: 'high' } },
    { seq: 7.5, at: '2026-11-02T10:46:00Z', type: 'HandoffCheckpointReached', thread: 'thread-a', data: { name: 'phase-commit', progress: { done: 2, total: 3 } } },
  ];
  const board = buildScoreboard([...extra, ...events], { observedAt: observed_at, days: 7, crates: crateGraph(METADATA) });
  assert.deepEqual(board.changes[0].classes, {
    plan_path: 'implementation plan',
    phases: '3',
    engine_or_frontend: 'engine',
    deepest_crate: 'example-core',
    kind: 'feat',
    agent: 'claude-code example-model high',
    started_by: 'human',
  });
});

test('a keep-alive turn with no model keeps the agent class whole', () => {
  const { events, observed_at } = load('worked-example');
  const settings = [
    { seq: 0.1, at: '2026-11-02T08:59:00Z', type: 'CodingAgentSettingsChanged', thread: 'thread-a', data: { coding_agent: 'claude-code', model: 'example-model', reasoning_effort: 'high' } },
    { seq: 7.5, at: '2026-11-02T10:46:00Z', type: 'CodingAgentSettingsChanged', thread: 'thread-a', data: { coding_agent: 'claude-code' } },
  ];
  const board = buildScoreboard([...settings, ...events], { observedAt: observed_at, days: 7 });
  assert.equal(board.changes[0].classes.agent, 'claude-code example-model high');
});

test('a later revert does not give a change the next change agent', () => {
  const { events, observed_at } = load('two-changes');
  const setting = (seq, model) => ({ seq, at: '2026-11-02T09:00:00Z', type: 'CodingAgentSettingsChanged', thread: 'thread-a', data: { coding_agent: 'claude-code', model } });
  const board = buildScoreboard([setting(0.5, 'model-one'), setting(6.5, 'model-two'), ...events], { observedAt: observed_at, days: 7 });
  assert.deepEqual(board.changes.map((r) => r.classes.agent), ['claude-code model-one', 'claude-code model-two']);
});

test('a plan-only Apply adds no zero to the rounds of its class', () => {
  const { events, observed_at } = load('plan-only-apply');
  const board = buildScoreboard(events, { observedAt: observed_at, days: 7 });
  assert.deepEqual(board.classes.plan_path['implementation plan'].rounds, { n: 0, tooFew: true });
});

test('a missing crate graph or file list is unknown, never a guess', () => {
  const { events, observed_at } = load('no-plan-fact');
  const board = buildScoreboard(events, { observedAt: observed_at, days: 7 });
  const classes = board.changes[0].classes;
  assert.equal(classes.deepest_crate, 'unknown');
  assert.equal(classes.plan_path, 'none');
  assert.equal(classes.kind, 'fix');
  assert.equal(classes.agent, 'unknown');
});

test('rounds per change count each round once, failed phases by name', () => {
  for (const name of ['harden-fails-and-returns', 'handoff-reemit']) {
    const { events, observed_at } = load(name);
    const board = buildScoreboard(events, { observedAt: observed_at, days: 7 });
    assert.equal(board.changes[0].rounds, 2, name);
  }
  const { events, observed_at } = load('handoff-reemit');
  const board = buildScoreboard(events, { observedAt: observed_at, days: 7 });
  assert.deepEqual(board.unit_costs.failed_phases, { '4.5 lint-rust': 1 });
  assert.deepEqual(board.unit_costs.round_seconds, { n: 2, tooFew: true, without_start: 0 });
});

test('build slot holds group by label, and a release without timings is unrecorded', () => {
  const at = '2026-11-02T10:00:00Z';
  const releases = [1, 2, 3, 4, 5].map((i) => ({
    seq: i, at, type: 'BuildSlotReleased', thread: null, data: { label: 'engine tests', wait_ms: i * 1000, hold_ms: i * 10_000, outcome: 'succeeded' },
  }));
  releases.push({ seq: 6, at, type: 'BuildSlotReleased', thread: null, data: { label: 'engine tests' } });
  const board = buildScoreboard(releases, { observedAt: '2026-11-02T12:00:00Z', days: 7 });
  assert.deepEqual(board.unit_costs.build_slot['engine tests'], {
    wait_ms: { n: 5, p50: 3000, p90: 4600 },
    hold_ms: { n: 5, p50: 30_000, p90: 46_000 },
    unrecorded: 1,
  });
});

test('engine builds and frontend refreshes report their seconds', () => {
  const { events, observed_at } = load('worked-example');
  const board = buildScoreboard(events, { observedAt: observed_at, days: 7 });
  assert.deepEqual(board.unit_costs.engine_builds, { succeeded: { n: 1, tooFew: true } });
  const other = load('other-repository');
  const refresh = buildScoreboard(other.events, { observedAt: other.observed_at, days: 7 }).unit_costs.frontend_refresh;
  assert.deepEqual(refresh, { n: 1, tooFew: true });
});

test('the window keeps changes that moved inside it', () => {
  const { events } = load('worked-example');
  const board = buildScoreboard(events, { observedAt: '2026-11-20T12:00:00Z', days: 7 });
  assert.equal(board.coverage.changes, 0);
});

test('rework counts reverts, failed applies and red main checks', () => {
  const { events, observed_at } = load('two-changes');
  const red = { seq: 99, at: '2026-11-02T13:00:00Z', type: 'MainCheckFailed', thread: null, data: {} };
  const board = buildScoreboard([...events, red], { observedAt: observed_at, days: 7 });
  assert.deepEqual(board.rework, { reverted: 1, apply_failed: 0, main_check_failed: 1 });
});

test('a revert inside the window counts even when its change landed before it', () => {
  const { events } = load('worked-example');
  const revert = { seq: 99, at: '2026-11-19T12:00:00Z', type: 'ChangeReverted', thread: 'thread-a', data: { change_id: 'change-a' } };
  const board = buildScoreboard([...events, revert], { observedAt: '2026-11-20T12:00:00Z', days: 7 });
  assert.equal(board.coverage.changes, 0);
  assert.equal(board.rework.reverted, 1);
});

test('suite timings read full rounds once, by shape and by suite', () => {
  const suites = { round: 'full', suites: { crates: { queued: 100, start: 110, end: 400, exit: 0 }, prose: { queued: 100, start: 100, end: 130, exit: 1 } } };
  const round = { at: '2026-11-02T10:00:00Z', type: 'HardenRunCompleted', thread: 'thread-a', data: { head_sha: 'aaaa', iteration: 1, verdict: 'fail', suites } };
  const fast = { ...round, seq: 3, data: { ...round.data, iteration: 2, suites: { ...suites, round: 'fast' } } };
  const timings = suiteTimings([{ ...round, seq: 1 }, { ...round, seq: 2 }, fast], {
    observedAt: '2026-11-02T12:00:00Z',
    days: 7,
    cargoSuites: ['crates'],
  });
  assert.deepEqual(timings.shapes, { 'cargo lane': { span_s: { n: 1, tooFew: true }, failing: 1, first_fail_to_verdict_s: { n: 1, tooFew: true } } });
  assert.deepEqual(Object.keys(timings.suites), ['crates', 'prose']);
  assert.equal(timings.suites.prose.failed, 1);
});
