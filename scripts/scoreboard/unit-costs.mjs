import { deriveChanges, dropReemits, mergeEvidence, USER_WAITS } from './stage-rules.mjs';
import { LUCIDOS_STAGE_MAP } from './lucidos-stage-map.mjs';

export const TOO_FEW_BELOW = 5;

export const SCOREBOARD_EVIDENCE = mergeEvidence(LUCIDOS_STAGE_MAP.evidence, {
  ChangeProposed: ['files'],
  ChangeApplied: ['commits'],
  HandoffCheckpointReached: ['name', 'progress'],
  CodingAgentSettingsChanged: ['coding_agent', 'model', 'reasoning_effort'],
  MessageReceived: ['mode'],
  BuildSlotReleased: ['label', 'wait_ms', 'hold_ms', 'outcome'],
  MainCheckFailed: [],
});

const COMMIT_KINDS = ['feat', 'fix', 'perf', 'refactor', 'test', 'docs', 'chore', 'build', 'ci', 'style', 'revert'];
const DAY_MS = 86_400_000;

const NOT_FAILED_EXITS = new Set([0, 72, 75]);

function landsCode(row) {
  return row.change.landedAt != null && row.change.content !== 'plan-only';
}

export function summarize(values) {
  const sorted = values.filter(Number.isFinite).sort((a, b) => a - b);
  const n = sorted.length;
  if (n < TOO_FEW_BELOW) return { n, tooFew: true };
  return { n, p50: percentile(sorted, 0.5), p90: percentile(sorted, 0.9) };
}

function percentile(sorted, q) {
  const rank = q * (sorted.length - 1);
  const lo = Math.floor(rank);
  const hi = Math.ceil(rank);
  return sorted[lo] + (sorted[hi] - sorted[lo]) * (rank - lo);
}

export function buildScoreboard(events, { observedAt, days, crates = null }) {
  const now = Date.parse(observedAt);
  const since = now - days * DAY_MS;
  const changes = deriveChanges(events, { map: LUCIDOS_STAGE_MAP, observedAt });
  const threads = threadContext(events);
  const inWindow = changes.filter((c) => endOf(c) >= since);
  const rows = inWindow.map((c) => changeRow(c, { threads, crates }));
  const landedCode = rows.filter(landsCode);
  const distinct = dropReemits(events, LUCIDOS_STAGE_MAP.reemitted);
  const after = (e) => Date.parse(e.at) >= since;
  return {
    observed_at: new Date(now).toISOString(),
    days,
    too_few_below: TOO_FEW_BELOW,
    coverage: coverage(rows),
    lead_time: summarize(known(rows).map((r) => r.change.leadTime.ms)),
    waits: summarize(known(rows).map((r) => r.change.leadTime.waitMs)),
    stages: stageTimes(rows),
    classes: classTable(rows),
    unit_costs: {
      rounds_per_change: summarize(landedCode.map((r) => r.rounds.length)),
      changes_without_rounds: landedCode.filter((r) => r.rounds.length === 0).length,
      failed_phases: countBy(rows.flatMap((r) => r.rounds.filter((x) => x.verdict === 'fail').map((x) => x.failed_phase || 'unnamed'))),
      round_seconds: roundSecondsSummary(rows),
      build_slot: slotHolds(distinct.filter((e) => e.type === 'BuildSlotReleased' && after(e))),
      engine_builds: engineBuilds(distinct.filter((e) => e.type === 'EngineBuildCompleted' && after(e))),
      frontend_refresh: summarize(rows.filter((r) => r.scope === 'frontend' && r.change.liveAt != null).map((r) => r.change.liveAt - r.change.landedAt)),
    },
    rework: {
      reverted: distinct.filter((e) => e.type === 'ChangeReverted' && after(e)).length,
      apply_failed: distinct.filter((e) => e.type === 'ChangeApplyFailed' && after(e)).length,
      main_check_failed: distinct.filter((e) => e.type === 'MainCheckFailed' && after(e)).length,
    },
    changes: rows.map(publicRow),
  };
}

function endOf(change) {
  return change.liveAt ?? change.landedAt ?? Date.parse(change.events.at(-1).at);
}

function known(rows) {
  return rows.filter((r) => r.change.leadTime.ms != null);
}

function coverage(rows) {
  return {
    changes: rows.length,
    lead_time_known: known(rows).length,
    lead_time_unknown: countBy(rows.filter((r) => r.change.leadTime.unknown).map((r) => r.change.leadTime.unknown)),
  };
}

function stageTimes(rows) {
  const stages = {};
  const slot = (name) => (stages[name] ||= { durations: [], unknown: 0, open: 0 });
  for (const { change } of rows) {
    for (const v of change.visits) {
      if (v.stage === 'live') continue;
      if (v.endUnknown) slot(v.stage).unknown++;
      else if (v.censored) slot(v.stage).open++;
      else if (v.end != null) slot(v.stage).durations.push(v.end - v.start);
    }
    for (const s of change.unknownStages) slot(s).unknown++;
  }
  const order = LUCIDOS_STAGE_MAP.stages.map((s) => s.name).filter((s) => stages[s]);
  return Object.fromEntries(order.map((s) => [s, { visits: summarize(stages[s].durations), unknown: stages[s].unknown, open: stages[s].open }]));
}

function changeRow(change, { threads, crates }) {
  const rounds = change.events.filter((e) => e.type === 'HardenRunCompleted').map((e) => e.data);
  const classes = classify(change, { threads, crates });
  return { change, rounds, roundSeconds: roundSeconds(change), scope: classes.engine_or_frontend, classes };
}

function publicRow({ change, rounds, classes }) {
  const visible = (ms) => (ms == null ? null : new Date(ms).toISOString());
  return {
    thread: change.thread,
    change_id: change.changeId,
    end: change.end,
    stage: change.visits.at(-1)?.stage ?? null,
    first_plan_fact: visible(change.firstPlanFact),
    landed_at: visible(change.landedAt),
    live_at: visible(change.liveAt),
    lead_time_ms: change.leadTime.ms ?? null,
    wait_ms: change.leadTime.waitMs ?? null,
    unknown: change.leadTime.unknown ?? null,
    hardening_ms: sumStage(change, 'hardening'),
    unknown_stages: change.unknownStages,
    rounds: rounds.length,
    waits: countBy(change.conditions.filter((c) => USER_WAITS.has(c.kind)).map((c) => c.kind)),
    classes,
  };
}

function sumStage(change, stage) {
  if (change.unknownStages.includes(stage)) return null;
  const visits = change.visits.filter((v) => v.stage === stage);
  if (!visits.length || visits.some((v) => v.end == null)) return null;
  return visits.reduce((t, v) => t + v.end - v.start, 0);
}

function roundSeconds(change) {
  const spans = [];
  let started = null;
  for (const e of change.events) {
    if (e.type === 'HardeningStarted') started = e;
    if (e.type === 'HardenRunCompleted' && started) {
      spans.push((Date.parse(e.at) - Date.parse(started.at)) / 1000);
      started = null;
    }
  }
  return spans;
}

function roundSecondsSummary(rows) {
  const spans = rows.flatMap((r) => r.roundSeconds);
  const rounds = rows.reduce((n, r) => n + r.rounds.length, 0);
  return { ...summarize(spans), without_start: rounds - spans.length };
}

function slotHolds(releases) {
  const byLabel = {};
  for (const { data } of releases) {
    const label = data.label || 'unlabelled';
    const entry = (byLabel[label] ||= { waits: [], holds: [], unrecorded: 0 });
    if (Number.isFinite(data.wait_ms) && Number.isFinite(data.hold_ms)) {
      entry.waits.push(data.wait_ms);
      entry.holds.push(data.hold_ms);
    } else {
      entry.unrecorded++;
    }
  }
  return Object.fromEntries(
    Object.keys(byLabel)
      .sort()
      .map((label) => {
        const { waits, holds, unrecorded } = byLabel[label];
        return [label, { wait_ms: summarize(waits), hold_ms: summarize(holds), unrecorded }];
      }),
  );
}

function engineBuilds(builds) {
  const byOutcome = {};
  for (const { data } of builds) (byOutcome[data.outcome || 'unknown'] ||= []).push(data.duration_ms);
  return Object.fromEntries(Object.keys(byOutcome).sort().map((o) => [o, summarize(byOutcome[o])]));
}

function threadContext(events) {
  const threads = new Map();
  const of = (id) => threads.get(id) || threads.set(id, { origin: null, settings: [] }).get(id);
  for (const e of [...events].sort((a, b) => a.seq - b.seq)) {
    if (e.thread == null) continue;
    if (e.type === 'MessageReceived' && of(e.thread).origin == null) of(e.thread).origin = e.data.mode ?? null;
    if (e.type === 'CodingAgentSettingsChanged') of(e.thread).settings.push(e);
  }
  return threads;
}

export function classify(change, { threads, crates }) {
  const thread = threads.get(change.thread) || { origin: null, settings: [] };
  const terminal = change.events.find((e) => (e.type === 'ChangeApplied' || e.type === 'ChangeDiscarded') && e.data.change_id === change.changeId);
  const lastSeq = (terminal ?? change.events.at(-1)).seq;
  const proposal = lastEvent(change.events, 'ChangeProposed');
  const applied = change.landedAt != null ? lastEvent(change.events, 'ChangeApplied') : null;
  const settings = thread.settings.filter((e) => e.seq <= lastSeq);
  const phases = change.events
    .filter((e) => e.type === 'HandoffCheckpointReached' && e.data.name === 'phase-commit')
    .map((e) => e.data.progress?.total)
    .filter(Number.isFinite);
  return {
    plan_path: planPath(change),
    phases: phases.length ? String(Math.max(...phases)) : 'none',
    engine_or_frontend: scopeOf(applied?.data, proposal?.data),
    deepest_crate: deepestCrate(proposal?.data.files, crates),
    kind: commitKind(applied?.data.commits),
    agent: agentOf(settings),
    started_by: thread.origin || 'unknown',
  };
}

function agentOf(settings) {
  const latest = (field) => settings.findLast((e) => e.data[field] != null)?.data[field];
  const parts = ['coding_agent', 'model', 'reasoning_effort'].map(latest).filter(Boolean);
  return parts.length ? parts.join(' ') : 'unknown';
}

function planPath(change) {
  const plans = change.events.filter((e) => e.type === 'PlanStarted' || e.type === 'PlanProposed' || e.type === 'PlanApproved');
  if (plans.some((e) => e.data.kind === 'design')) return 'design first';
  return plans.length ? 'implementation plan' : 'none';
}

function scopeOf(applied, proposal) {
  if (applied) return applied.requires_restart ? 'engine' : applied.client_update ? 'frontend' : 'neither';
  return proposal?.requires_restart ? 'engine' : 'unknown';
}

function deepestCrate(files, crates) {
  if (!crates || !Array.isArray(files)) return 'unknown';
  const touched = [...new Set(files.map((f) => crates.crateOf(f)).filter(Boolean))];
  if (!touched.length) return 'none';
  return touched.sort((a, b) => crates.dependents(b) - crates.dependents(a) || a.localeCompare(b))[0];
}

function commitKind(commits) {
  if (!Array.isArray(commits)) return 'unknown';
  const kinds = new Set(commits.map((s) => /^(\w+)(\([^)]*\))?!?:/.exec(s)?.[1]).filter(Boolean));
  return COMMIT_KINDS.find((k) => kinds.has(k)) || 'other';
}

export function crateGraph(metadata) {
  const root = metadata.workspace_root.replace(/\/$/, '');
  const members = metadata.packages.map((p) => ({
    name: p.name,
    dir: p.manifest_path.slice(root.length + 1).replace(/\/?Cargo\.toml$/, ''),
    deps: p.dependencies.filter((d) => d.path).map((d) => d.name),
  }));
  const names = new Set(members.map((m) => m.name));
  const dependentsOf = new Map(members.map((m) => [m.name, new Set()]));
  for (const m of members) for (const d of m.deps) if (names.has(d)) dependentsOf.get(d).add(m.name);
  const reach = new Map();
  const count = (name) => {
    if (reach.has(name)) return reach.get(name);
    const seen = new Set();
    const stack = [...dependentsOf.get(name)];
    while (stack.length) {
      const next = stack.pop();
      if (seen.has(next)) continue;
      seen.add(next);
      stack.push(...dependentsOf.get(next));
    }
    reach.set(name, seen.size);
    return seen.size;
  };
  const byDepth = members.filter((m) => m.dir).sort((a, b) => b.dir.length - a.dir.length);
  return {
    crateOf: (file) => byDepth.find((m) => file === m.dir || file.startsWith(`${m.dir}/`))?.name ?? null,
    dependents: count,
  };
}

function classTable(rows) {
  const table = {};
  for (const r of rows) {
    for (const [attribute, value] of Object.entries(r.classes)) {
      ((table[attribute] ||= {})[value] ||= []).push(r);
    }
  }
  return Object.fromEntries(
    Object.entries(table).map(([attribute, values]) => [
      attribute,
      Object.fromEntries(
        Object.keys(values)
          .sort()
          .map((value) => [
            value,
            {
              changes: values[value].length,
              lead_time: summarize(known(values[value]).map((r) => r.change.leadTime.ms)),
              rounds: summarize(values[value].filter(landsCode).map((r) => r.rounds.length)),
            },
          ]),
      ),
    ]),
  );
}

export function suiteTimings(events, { observedAt, days, cargoSuites }) {
  const since = Date.parse(observedAt) - days * DAY_MS;
  const cargo = new Set(cargoSuites);
  const rounds = dropReemits(events, LUCIDOS_STAGE_MAP.reemitted)
    .filter((e) => e.type === 'HardenRunCompleted' && Date.parse(e.at) >= since)
    .map((e) => e.data.suites)
    .filter((s) => s && s.round === 'full' && s.suites && typeof s.suites === 'object')
    .map((s) =>
      Object.entries(s.suites)
        .filter(([, v]) => v.end != null && v.exit !== '' && v.exit != null)
        .map(([suite, v]) => ({ suite, queued: v.queued, start: v.start, end: v.end, failed: !NOT_FAILED_EXITS.has(Number(v.exit)) })),
    )
    .filter((suites) => suites.length);
  const shapes = {};
  for (const suites of rounds) {
    const shape = suites.some((s) => s.suite.startsWith('e2e-touched'))
      ? 'browser leg'
      : suites.some((s) => cargo.has(s.suite))
        ? 'cargo lane'
        : 'no cargo lane';
    const entry = (shapes[shape] ||= { spans: [], failing: 0, failToVerdict: [] });
    const end = Math.max(...suites.map((s) => s.end));
    entry.spans.push(end - Math.min(...suites.map((s) => s.queued)));
    const failed = suites.filter((s) => s.failed);
    if (failed.length) {
      entry.failing++;
      entry.failToVerdict.push(end - Math.min(...failed.map((s) => s.end)));
    }
  }
  const perSuite = {};
  for (const s of rounds.flat()) {
    const entry = (perSuite[s.suite] ||= { runs: [], waits: [], failed: 0 });
    entry.runs.push(s.end - s.start);
    entry.waits.push(s.start - s.queued);
    if (s.failed) entry.failed++;
  }
  return {
    shapes: Object.fromEntries(
      Object.entries(shapes).map(([shape, e]) => [shape, { span_s: summarize(e.spans), failing: e.failing, first_fail_to_verdict_s: summarize(e.failToVerdict) }]),
    ),
    suites: Object.fromEntries(
      Object.keys(perSuite)
        .sort()
        .map((suite) => [suite, { run_s: summarize(perSuite[suite].runs), wait_s: summarize(perSuite[suite].waits), failed: perSuite[suite].failed }]),
    ),
  };
}

function countBy(values) {
  const counts = {};
  for (const v of values) counts[v] = (counts[v] || 0) + 1;
  return counts;
}

function lastEvent(events, type) {
  return events.findLast((e) => e.type === type);
}
