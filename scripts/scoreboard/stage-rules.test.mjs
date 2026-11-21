// The stage rules against their fixtures. Run: node --test scripts/scoreboard/
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

// A factory that is not Lucidos: one stage of its own, and no Hardening.
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
    landed(1, '2026-11-02T10:00:00Z', 'thread-a', '1'),
    { seq: 2, at: '2026-11-02T10:10:00Z', type: 'EngineVersionActivated', thread: null, data: { build_id: '0000000000' } },
    landed(3, '2026-11-02T10:20:00Z', 'thread-b', '2'),
    { seq: 4, at: '2026-11-02T10:30:00Z', type: 'EngineVersionActivated', thread: null, data: { build_id: '2222222222' } },
  ];
  const changes = deriveChanges(events, { map: ENGINE_STAGE_MAP, observedAt: '2026-11-02T11:00:00Z' });
  assert.deepEqual(changes.map((c) => c.liveAt), [Date.parse('2026-11-02T10:30:00Z'), Date.parse('2026-11-02T10:30:00Z')]);
});

test("another repository's factory adds its own stage and needs no Hardening", () => {
  const [change] = derive('other-repository', REVIEW_MAP);
  assert.deepEqual(stagesOf(change), ['planning', 'implementing', 'review', 'ready-to-land', 'landing', 'landed', 'live']);
  assert.deepEqual(change.unknownStages, []);
  assert.equal(change.planKind, 'implementation');
  assert.equal(change.planBranch, 'feature');
  assert.equal(minutes(change.visits.find((v) => v.stage === 'review')), 20);
  // 09:00 to 10:54, minus 10 minutes of plan approval and 30 of Apply.
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
  // Each minus its own Apply wait: 10 minutes, then 20.
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
