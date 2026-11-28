import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { deriveChanges, ENGINE_STAGE_MAP, extendStageMap, mergeEvidence, unionLength } from './stage-rules.mjs';
import { LUCIDOS_STAGE_MAP } from './lucidos-stage-map.mjs';
import { SCOREBOARD_EVIDENCE } from './unit-costs.mjs';

const FIXTURES = new URL('./fixtures/', import.meta.url);
const MIN = 60_000;

function load(name) {
  return JSON.parse(readFileSync(new URL(`${name}.json`, FIXTURES), 'utf8'));
}

function derive(name, map = LUCIDOS_STAGE_MAP) {
  const fixture = load(name);
  return deriveChanges(fixture.events, { map, observedAt: fixture.observed_at });
}

function minutes(visit) {
  return (visit.end - visit.start) / MIN;
}

function stagesOf(change) {
  return change.visits.map((v) => v.stage);
}

const REVIEW_MAP = extendStageMap(ENGINE_STAGE_MAP, {
  stages: [{ name: 'review', after: 'implementing' }],
  rules: [{ on: 'ReviewStarted', to: 'review' }],
  evidence: { ReviewStarted: ['reviewer'] },
});

test("the design's worked example: 1 h 40 min of lead time, 45 minutes of Hardening", () => {
  const [change] = derive('worked-example');
  assert.equal(change.leadTime.ms, 100 * MIN);
  assert.equal(change.leadTime.waitMs, 62 * MIN);
  assert.equal(change.leadTime.spanMs, 162 * MIN);
  const hardening = change.visits.filter((v) => v.stage === 'hardening');
  assert.deepEqual(hardening.map(minutes), [45]);
  assert.deepEqual(stagesOf(change), ['planning', 'implementing', 'hardening', 'ready-to-land', 'landed', 'live']);
  assert.deepEqual(
    change.conditions.map((c) => [c.kind, (c.end - c.start) / MIN]),
    [['plan-approval', 8], ['question', 10], ['apply', 40], ['switch', 4]],
  );
});

test('the rules are a pure function of the events: two runs agree', () => {
  for (const file of readdirSync(FIXTURES)) {
    const fixture = JSON.parse(readFileSync(new URL(file, FIXTURES), 'utf8'));
    const map = file.startsWith('other-repository') ? REVIEW_MAP : LUCIDOS_STAGE_MAP;
    const run = () => deriveChanges(structuredClone(fixture.events), { map, observedAt: fixture.observed_at });
    assert.deepEqual(run(), run(), file);
  }
});

test('the rules need an observation time and read no clock', () => {
  assert.throws(() => deriveChanges([], { map: LUCIDOS_STAGE_MAP }), /observation time/);
});

test('the rules module names no Lucidos event', () => {
  const source = readFileSync(new URL('./stage-rules.mjs', import.meta.url), 'utf8');
  for (const type of Object.keys(LUCIDOS_STAGE_MAP.evidence).filter((t) => !ENGINE_STAGE_MAP.evidence[t])) {
    assert.ok(!source.includes(type), `stage-rules.mjs names ${type}`);
  }
  assert.ok(!source.includes("'hardening'"));
});

test('a later landing in a running build makes an earlier change Live', () => {
  const sha = (digit) => digit.repeat(40);
  const landed = (seq, at, thread, digit) => ({
    seq, at, type: 'ChangeApplied', thread,
    data: { change_id: `change-${digit}`, content: 'code', requires_restart: true, post_merge_sha: sha(digit), landing_merge_sha: sha(digit) },
  });
  const events = [
    { seq: -1, at: '2026-11-02T09:58:00Z', type: 'EngineVersionActivated', thread: null, data: { build_id: '000000000000' } },
    { seq: 0, at: '2026-11-02T09:59:00Z', type: 'ChangeApplyRequested', thread: 'thread-a', data: { change_id: 'change-1', repo_root: '/work/example-repo' } },
    landed(1, '2026-11-02T10:00:00Z', 'thread-a', '1'),
    { seq: 2, at: '2026-11-02T10:10:00Z', type: 'EngineVersionActivated', thread: null, data: { build_id: '0000000000' } },
    { seq: 2.5, at: '2026-11-02T10:19:00Z', type: 'ChangeApplyRequested', thread: 'thread-b', data: { change_id: 'change-2', repo_root: '/work/example-repo' } },
    landed(3, '2026-11-02T10:20:00Z', 'thread-b', '2'),
    { seq: 4, at: '2026-11-02T10:30:00Z', type: 'EngineVersionActivated', thread: null, data: { build_id: '2222222222' } },
  ];
  const changes = deriveChanges(events, { map: ENGINE_STAGE_MAP, observedAt: '2026-11-02T11:00:00Z' });
  assert.deepEqual(changes.map((c) => c.liveAt), [Date.parse('2026-11-02T10:30:00Z'), Date.parse('2026-11-02T10:30:00Z')]);
});

function rolloutEvent(seq, type, data, thread = null) {
  return { seq, at: seq * MIN, type, thread, data };
}

function landing(seq, digit, repoRoot, flags) {
  const thread = `thread-${digit}`;
  const change_id = `change-${digit}`;
  return [
    rolloutEvent(seq - 2, 'PlanApproved', { kind: 'simple-fix' }, thread),
    rolloutEvent(seq - 1, 'ChangeApplyRequested', { change_id, repo_root: repoRoot }, thread),
    rolloutEvent(seq, 'ChangeApplied', {
      change_id, content: 'code', landing_merge_sha: digit.repeat(40), ...flags,
    }, thread),
  ];
}

function rolloutChanges(events) {
  return deriveChanges(events, { map: ENGINE_STAGE_MAP, observedAt: 100 * MIN });
}

const activate = (seq, engine, frontend) => rolloutEvent(seq, 'EngineVersionActivated', {
  build_id: engine, frontend_source_commit: frontend,
});
const serve = (seq, commit) => rolloutEvent(seq, 'ServedFrontendCommitChanged', { source_commit: commit });

for (const [surface, flags, match] of [
  ['engine', { requires_restart: true }, (seq) => activate(seq, '1111111111')],
  ['frontend', { client_update: true }, (seq) => serve(seq, '1111111111')],
]) {
  test(`the first ${surface} observation does not prove when a historical landing went live`, () => {
    const [change] = rolloutChanges([
      surface === 'engine' ? serve(1, '0000000000') : rolloutEvent(1, 'EngineBuildCompleted', {}),
      ...landing(4, '1', '/work/example-repo', flags),
      match(20),
    ]);
    assert.equal(change.firstObservedLiveAt, 20 * MIN);
    assert.equal(change.liveAt, null);
    assert.deepEqual(change.leadTime, { unknown: 'live-unproven' });
    assert.deepEqual(change.visits.at(-1), { stage: 'landed', start: 4 * MIN, end: null, endUnknown: true });
  });

  test(`a batch of three ${surface} changes shares a proven rollout time`, () => {
    const repo = '/work/example-repo';
    const changes = rolloutChanges([
      surface === 'engine' ? activate(1, '0000000000') : serve(1, '0000000000'),
      ...landing(4, '1', repo, flags),
      ...landing(7, '2', repo, flags),
      ...landing(10, '3', repo, flags),
      surface === 'engine' ? activate(20, '3333333333') : activate(20, '0000000000', '3333333333'),
    ]);
    assert.deepEqual(changes.map((c) => c.liveAt), [20 * MIN, 20 * MIN, 20 * MIN]);
    assert.deepEqual(changes.map((c) => c.firstObservedLiveAt), [20 * MIN, 20 * MIN, 20 * MIN]);
  });
}

test('an unresolvable activation after Apply leaves the first engine match unproven', () => {
  const [change] = rolloutChanges([
    activate(1, '0000000000'),
    ...landing(4, '1', '/work/example-repo', { requires_restart: true }),
    activate(5, 'unknown-build'),
    activate(6, '1111111111'),
    activate(7, '1111111111'),
  ]);
  assert.equal(change.firstObservedLiveAt, 6 * MIN);
  assert.equal(change.liveAt, null);
  assert.deepEqual(change.leadTime, { unknown: 'live-unproven' });
});

test('only observations after Apply need a resolvable commit', () => {
  const [change] = rolloutChanges([
    activate(1, 'src-0123456789abcdef'),
    ...landing(4, '1', '/work/example-repo', { requires_restart: true }),
    activate(5, '1111111111'),
  ]);
  assert.equal(change.liveAt, 5 * MIN);
});

for (const [label, priorLanding] of [
  ['a direct commit', []],
  ['a landing with no repository', landing(4, '2', undefined, {})],
  ['a landing in another repository', landing(4, '2', '/work/other-repo', {})],
  ['a landing in the same repository', landing(4, '2', '/work/example-repo', {})],
]) {
  test(`an intervening activation of ${label} needs pre-Apply evidence in the change's repository or surface`, () => {
    const changes = rolloutChanges([
      activate(1, '0000000000'),
      serve(5, '2222222222'),
      ...priorLanding,
      ...landing(8, '1', '/work/example-repo', { requires_restart: true }),
      activate(9, '2222222222'),
      activate(10, '1111111111'),
    ]);
    const change = changes.find((entry) => entry.changeId === 'change-1');
    assert.equal(change.firstObservedLiveAt, 10 * MIN);
    assert.equal(change.liveAt, label === 'a landing in the same repository' ? 10 * MIN : null);
  });
}

test('the first activation can supply a baseline when its commit was landed before Apply', () => {
  const repo = '/work/example-repo';
  const [, change] = rolloutChanges([
    ...landing(4, '2', repo, {}),
    ...landing(8, '1', repo, { requires_restart: true }),
    activate(9, '2222222222'),
    activate(10, '1111111111'),
  ]);
  assert.equal(change.liveAt, 10 * MIN);
});

test('a frontend snapshot can identify a version observed on the frontend before Apply', () => {
  const [change] = rolloutChanges([
    activate(1, '0000000000', '2222222222222222'),
    ...landing(4, '1', '/work/example-repo', { client_update: true }),
    serve(5, '2222222222'),
    serve(6, '1111111111'),
  ]);
  assert.equal(change.liveAt, 6 * MIN);
});

test('a frontend snapshot with no source commit after Apply makes rollout unproven', () => {
  const [change] = rolloutChanges([
    serve(1, '0000000000'),
    ...landing(4, '1', '/work/example-repo', { client_update: true }),
    activate(5, '0000000000'),
    serve(6, '1111111111'),
  ]);
  assert.equal(change.firstObservedLiveAt, 6 * MIN);
  assert.deepEqual(change.leadTime, { unknown: 'live-unproven' });
});

test('a revert before the matching rollout leaves rollout unknown', () => {
  const [change] = rolloutChanges([
    serve(1, '0000000000'),
    ...landing(4, '1', '/work/example-repo', { client_update: true }),
    rolloutEvent(5, 'ChangeReverted', { change_id: 'change-1' }, 'thread-1'),
    serve(6, '1111111111'),
  ]);
  assert.equal(change.liveAt, null);
  assert.deepEqual(change.leadTime, { unknown: 'live-unproven' });
  assert.equal(change.visits.at(-1).endUnknown, true);
});

test('a revert after the matching rollout preserves proven first live', () => {
  const [change] = rolloutChanges([
    serve(1, '0000000000'),
    ...landing(4, '1', '/work/example-repo', { client_update: true }),
    serve(5, '1111111111'),
    rolloutEvent(6, 'ChangeReverted', { change_id: 'change-1' }, 'thread-1'),
  ]);
  assert.equal(change.liveAt, 5 * MIN);
  assert.equal(change.firstObservedLiveAt, 5 * MIN);
});

test('only a landing from the same known repository can stand in for a change', () => {
  for (const [firstRepo, laterRepo] of [
    ['/work/one', '/work/two'], ['/work/one', undefined], [undefined, undefined], [undefined, '/work/two'],
  ]) {
    const changes = rolloutChanges([
      activate(1, '0000000000'),
      ...landing(4, '1', firstRepo, { requires_restart: true }),
      ...landing(7, '2', laterRepo, { requires_restart: true }),
      activate(10, '2222222222'),
    ]);
    assert.equal(changes[0].firstObservedLiveAt, null);
    assert.equal(changes[0].liveAt, null);
    assert.equal(changes[1].liveAt, 10 * MIN, 'the own landing matches without repository evidence');
  }
});

test("another repository's factory adds its own stage and needs no Hardening", () => {
  const [change] = derive('other-repository', REVIEW_MAP);
  assert.deepEqual(stagesOf(change), ['planning', 'implementing', 'review', 'ready-to-land', 'landing', 'landed', 'live']);
  assert.deepEqual(change.unknownStages, []);
  assert.equal(change.planKind, 'implementation');
  assert.equal(change.planBranch, 'feature');
  assert.equal(minutes(change.visits.find((v) => v.stage === 'review')), 20);
  assert.equal(change.leadTime.ms, (114 - 40) * MIN);
});

test('the engine map alone resolves every stage without Hardening', () => {
  const [change] = derive('worked-example', ENGINE_STAGE_MAP);
  assert.ok(!stagesOf(change).includes('hardening'));
  assert.deepEqual(change.unknownStages, []);
  assert.equal(change.leadTime.ms, 100 * MIN);
});

test('missing evidence is unknown, never zero', () => {
  const [a, b] = derive('missing-facts');
  assert.deepEqual(a.unknownStages, ['hardening']);
  const implementing = a.visits.find((v) => v.stage === 'implementing');
  assert.equal(implementing.end, null, 'Implementing has no evidenced end');
  assert.equal(implementing.endUnknown, true);
  assert.deepEqual(a.leadTime, { unknown: 'not-live' });
  assert.equal(a.visits.at(-1).stage, 'landed');
  assert.equal(a.visits.at(-1).censored, true);

  assert.deepEqual(b.unknownStages, []);
  assert.equal(b.liveAt, Date.parse('2026-11-02T12:30:00Z'), 'a dirty build id still names its commit');
  assert.deepEqual(b.leadTime, { unknown: 'unclosed-switch' });
});

test('overlapping waits on the user count once', () => {
  const [change] = derive('overlapping-waits');
  assert.equal(change.leadTime.waitMs, 20 * MIN);
  assert.equal(change.leadTime.ms, 45 * MIN);
  assert.ok(!change.conditions.some((c) => c.kind === 'apply'), 'a standing Apply covers the Apply wait');
});

test('a plan-only Apply moves the stage and never reaches Landed', () => {
  const [implementation, design] = derive('plan-only-apply');
  assert.deepEqual(stagesOf(implementation), ['planning', 'ready-to-build', 'ready-to-land', 'ready-to-build']);
  assert.deepEqual(stagesOf(design), ['designing', 'ready-to-land', 'planning']);
  for (const change of [implementation, design]) {
    assert.equal(change.content, 'plan-only');
    assert.deepEqual(change.leadTime, { unknown: 'plan-only' });
  }
});

test('a hardening round that fails returns the change to Implementing', () => {
  const [change] = derive('harden-fails-and-returns');
  assert.deepEqual(stagesOf(change), ['implementing', 'hardening', 'implementing', 'hardening', 'ready-to-land']);
  assert.deepEqual(change.visits.slice(1, 4).map(minutes), [20, 10, 25]);
  const last = change.visits.at(-1);
  assert.equal(last.censored, true, 'the current visit is censored at observation time');
  assert.equal(minutes(last), 35);
  const apply = change.conditions.find((c) => c.kind === 'apply');
  assert.equal(apply.censored, true);
  assert.deepEqual(change.leadTime, { unknown: 'not-live' });
});

test('a change with no plan fact is labelled as that', () => {
  const [change] = derive('no-plan-fact');
  assert.equal(change.firstPlanFact, null);
  assert.deepEqual(change.leadTime, { unknown: 'no-plan-fact' });
  assert.deepEqual(change.unknownStages, ['implementing']);
  assert.equal(change.liveAt, Date.parse('2026-11-02T10:30:00Z'), 'nothing to roll out: live once landed');
});

test('a handoff re-emit counts once', () => {
  const [change] = derive('handoff-reemit');
  assert.deepEqual(stagesOf(change), ['implementing', 'hardening', 'implementing', 'hardening', 'ready-to-land']);
  assert.equal(change.events.filter((e) => e.type === 'HardenRunCompleted').length, 2);
  assert.equal(change.events.filter((e) => e.type === 'HardeningStarted').length, 2);
});

test('rework after a hardened proposal returns to Implementing and is not an Apply wait', () => {
  const [change] = derive('rework-after-proposal');
  assert.deepEqual(stagesOf(change), ['implementing', 'hardening', 'ready-to-land', 'implementing', 'hardening', 'ready-to-land', 'live']);
  const waits = change.conditions.filter((c) => c.kind === 'apply').map((c) => (c.end - c.start) / MIN);
  assert.deepEqual(waits, [10, 5], 'your reply ends the first wait');
  assert.equal(change.leadTime.ms, 160 * MIN);
});

test('dropping a standing Apply while Ready to land starts the Apply wait', () => {
  const event = (seq, time, type, data = {}) => ({ seq, at: `2026-11-02T${time}:00Z`, type, thread: 'thread-a', data });
  const events = [
    event(1, '09:00', 'PlanApproved', { kind: 'implementation', scope: 'implement' }),
    event(2, '09:30', 'HardeningStarted', { head_sha: 'aaaa', iteration: 1, tier: 'full' }),
    event(3, '09:40', 'StandingApplyArmed'),
    event(4, '10:00', 'ChangeProposed', { change_id: 'change-a', content: 'code', requires_restart: false, hardened: true }),
    event(5, '10:30', 'StandingApplyDropped'),
    event(6, '10:50', 'ChangeApplied', { change_id: 'change-a', content: 'code', requires_restart: false, client_update: false }),
  ];
  const [change] = deriveChanges(events, { map: LUCIDOS_STAGE_MAP, observedAt: '2026-11-02T12:00:00Z' });
  const waits = change.conditions.filter((c) => c.kind === 'apply').map((c) => (c.end - c.start) / MIN);
  assert.deepEqual(waits, [20]);
  assert.equal(change.leadTime.ms, 90 * MIN);
});

test("a thread's second change is its own unit", () => {
  const changes = derive('two-changes');
  assert.deepEqual(changes.map((c) => c.changeId), ['change-1', 'change-2']);
  const [first, second] = changes;
  assert.equal(first.firstPlanFact, Date.parse('2026-11-02T09:00:00Z'));
  assert.equal(second.firstPlanFact, Date.parse('2026-11-02T11:00:00Z'));
  assert.equal(first.leadTime.ms, 60 * MIN);
  assert.equal(second.leadTime.ms, 70 * MIN);
  assert.ok(first.events.some((e) => e.type === 'ChangeReverted'), 'the revert joins the change it reverts');
  assert.ok(!second.events.some((e) => e.type === 'ChangeReverted'));
});

test('fixtures carry only the fields the scoreboard fetches', () => {
  const evidence = mergeEvidence(SCOREBOARD_EVIDENCE, REVIEW_MAP.evidence);
  for (const file of readdirSync(FIXTURES)) {
    const { events } = JSON.parse(readFileSync(new URL(file, FIXTURES), 'utf8'));
    for (const e of events) {
      assert.ok(evidence[e.type], `${file}: ${e.type} is not fetched`);
      for (const key of Object.keys(e.data)) {
        assert.ok(evidence[e.type].includes(key), `${file}: ${e.type}.${key} is not fetched`);
      }
    }
  }
});

test('a stage map rule naming an unknown stage fails loudly', () => {
  const broken = extendStageMap(ENGINE_STAGE_MAP, { rules: [{ on: 'PlanStarted', to: 'nowhere' }] });
  const events = [{ seq: 1, at: '2026-11-02T09:00:00Z', type: 'PlanStarted', thread: 't', data: { change_id: 'c' } }];
  assert.throws(() => deriveChanges(events, { map: broken, observedAt: '2026-11-02T10:00:00Z' }), /unknown stage/);
});

test('union length merges overlapping intervals', () => {
  assert.equal(unionLength([[0, 10], [5, 15], [20, 25], [21, 22]]), 20);
  assert.equal(unionLength([]), 0);
});

test('a fix marker starts implementation without a human approval wait', () => {
  const fixture = load('other-repository');
  const events = fixture.events.filter((e) => e.seq > 2);
  events[0].data.kind = 'simple-fix';
  const [change] = deriveChanges(events, { map: REVIEW_MAP, observedAt: fixture.observed_at });
  assert.equal(change.planKind, 'simple-fix');
  assert.equal(change.visits[0].stage, 'implementing');
  assert.equal(change.conditions.some((c) => c.kind === 'plan-approval'), false);
});
