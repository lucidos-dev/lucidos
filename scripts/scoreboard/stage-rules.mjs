// The stage rules: one pure function from a change's events to its stage
// visits, conditions and lead time (docs/plans/2026-11-19-scoreboard-2-stage-rules-and-script.md).
//
// Plain JavaScript with no imports, so the Software Factory app can ship a
// checked copy. It reads no clock, file or network: the caller passes the
// observation time. It names only engine facts. A factory adds its own stages
// through a stage map, as lucidos-stage-map.mjs does for Hardening.
//
// An event is { seq, at, type, thread, data }. `seq` is the events table's
// sequence and `at` its ISO time. `thread` is null for a global fact. `data`
// holds the payload fields named in the map's evidence.

// The payload fields each engine fact contributes. The scoreboard fetches
// exactly these, so a rule that reads an undeclared field sees nothing.
export const ENGINE_EVIDENCE = {
  PlanStarted: ['kind'],
  PlanProposed: ['kind'],
  PlanApproved: ['kind', 'scope', 'branch_name'],
  UserQuestionAsked: ['tool_use_id', 'permission_request'],
  UserQuestionAnswered: ['tool_use_id'],
  CodingAgentPermissionRequest: ['request_id'],
  CodingAgentPermissionResolved: ['request_id'],
  ProposalWithheld: ['reason'],
  MessageReceived: ['mode'],
  ChangeProposed: ['change_id', 'content', 'requires_restart'],
  ChangeQueued: ['change_id'],
  ApplyNowStarted: [],
  ChangeApplyRequested: ['change_id', 'branch_name', 'repo_root'],
  ChangeUnqueued: ['change_id'],
  ChangeApplyFailed: ['change_id'],
  ChangeApplied: ['change_id', 'content', 'requires_restart', 'client_update', 'post_merge_sha', 'landing_merge_sha'],
  ChangeDiscarded: ['change_id'],
  ChangeReverted: ['change_id'],
  StandingApplyArmed: [],
  StandingApplyDropped: [],
  StandingApplyFired: ['change_id'],
  EngineBuildCompleted: ['outcome', 'duration_ms', 'built_head'],
  EngineVersionActivated: ['build_id', 'frontend_source_commit'],
  ServedFrontendCommitChanged: ['source_commit'],
};

const PLAN_FACTS = new Set(['PlanStarted', 'PlanProposed', 'PlanApproved']);
const TERMINALS = new Set(['ChangeApplied', 'ChangeDiscarded']);

// The waits on the user. Lead time removes their union, so overlaps count once.
export const USER_WAITS = new Set(['plan-approval', 'question', 'permission', 'apply', 'switch']);

// A stage map: the stages in order, and the rules that move a change between
// them. The first rule matching an event decides. A rule's `to` is a stage, a
// function of (data, unit) returning one, or null to stay put. An `expected`
// stage that a change passes over is reported as unknown, never as zero.
export const ENGINE_STAGE_MAP = {
  stages: [
    { name: 'designing' },
    { name: 'planning' },
    { name: 'ready-to-build' },
    { name: 'implementing', expected: true },
    { name: 'ready-to-land' },
    { name: 'landing' },
    { name: 'landed' },
    { name: 'live' },
  ],
  rules: [
    { on: 'PlanStarted', to: (d) => (d.kind === 'design' ? 'designing' : 'planning') },
    { on: 'PlanApproved', to: (d) => (d.scope !== 'plan-only' ? 'implementing' : planOnlyStage(d.kind)) },
    { on: 'ChangeProposed', to: 'ready-to-land' },
    { on: 'ChangeApplyRequested', to: 'landing' },
    { on: 'ChangeQueued', to: 'landing' },
    { on: 'ApplyNowStarted', to: 'landing' },
    { on: 'ChangeUnqueued', to: 'ready-to-land' },
    { on: 'ChangeApplyFailed', to: 'ready-to-land' },
    { on: 'ChangeApplied', to: (d, unit) => (d.content === 'plan-only' ? planOnlyStage(unit.planKind) : 'landed') },
  ],
  // Event types a handoff can record twice, each with the payload fields that
  // identify one record. A copy with the same fields is dropped.
  reemitted: {},
  evidence: ENGINE_EVIDENCE,
};

// A plan-only Apply moves a design to Planning and an implementation plan to
// Ready to build. It never reaches Landed.
function planOnlyStage(kind) {
  return kind === 'design' ? 'planning' : 'ready-to-build';
}

// Extend the engine map with a factory's own stages and rules. Each added
// stage names the stage it follows. Factory rules run before engine rules.
export function extendStageMap(base, { stages = [], rules = [], reemitted = {}, evidence = {} }) {
  const order = base.stages.slice();
  for (const { after, ...stage } of stages) {
    const at = order.findIndex((s) => s.name === after);
    if (at < 0) throw new Error(`stage map: no stage '${after}' to follow`);
    order.splice(at + 1, 0, stage);
  }
  return {
    stages: order,
    rules: [...rules, ...base.rules],
    reemitted: { ...base.reemitted, ...reemitted },
    evidence: mergeEvidence(base.evidence, evidence),
  };
}

// The union of several evidence declarations, field by field.
export function mergeEvidence(...declarations) {
  const merged = {};
  for (const declaration of declarations) {
    for (const [type, fields] of Object.entries(declaration)) {
      merged[type] = [...new Set([...(merged[type] || []), ...fields])];
    }
  }
  return merged;
}

// Derive every change's stages, conditions and lead time from the complete
// event history. A change is one thread's change id.
export function deriveChanges(events, { map, observedAt }) {
  if (!map) throw new Error('deriveChanges needs a stage map');
  const now = toMs(observedAt);
  if (!Number.isFinite(now)) throw new Error('deriveChanges needs an observation time');
  const ordered = dropReemits(events, map.reemitted).sort((a, b) => a.seq - b.seq);
  const threads = new Map();
  const global = [];
  for (const event of ordered) {
    if (event.thread == null) global.push(event);
    else (threads.get(event.thread) || threads.set(event.thread, []).get(event.thread)).push(event);
  }
  const landings = ordered.filter((e) => e.type === 'ChangeApplied');
  const units = [];
  for (const [thread, stream] of threads) {
    const armed = armedIntervals(stream);
    for (const unit of splitUnits(thread, stream)) {
      units.push(deriveUnit(unit, { map, now, armed, global, landings }));
    }
  }
  return units.sort((a, b) => a.events[0].seq - b.events[0].seq);
}

// Drop the copies a handoff re-emitted: same type, thread and key fields.
export function dropReemits(events, reemitted) {
  const seen = new Set();
  return events.filter((event) => {
    const fields = reemitted[event.type];
    if (!fields) return true;
    const identity = Object.fromEntries(fields.map((f) => [f, event.data[f] ?? null]));
    const key = `${event.type}\u0000${event.thread}\u0000${canonical(identity)}`;
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((k) => `${JSON.stringify(k)}:${canonical(value[k])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}

// A thread's events split into its changes. A change runs from the end of the
// previous one to its own Apply or discard. Evidence naming a change id that
// already ended, such as a revert, joins that change.
function splitUnits(thread, stream) {
  const units = [];
  const byChange = new Map();
  let current = newUnit(thread);
  for (const event of stream) {
    const id = event.data.change_id;
    const ended = id != null ? byChange.get(id) : undefined;
    if (ended && ended !== current) {
      ended.events.push(event);
      continue;
    }
    if (id != null && current.changeId == null) {
      current.changeId = id;
      byChange.set(id, current);
    } else if (id != null && current.changeId !== id) {
      current.end = 'superseded';
      units.push(current);
      current = newUnit(thread);
      current.changeId = id;
      byChange.set(id, current);
    }
    current.events.push(event);
    if (TERMINALS.has(event.type) && id === current.changeId) {
      current.end = event.type === 'ChangeApplied' ? 'applied' : 'discarded';
      units.push(current);
      current = newUnit(thread);
    }
  }
  // Work that never became a change is not a unit yet.
  if (current.changeId != null) units.push(current);
  return units;
}

function newUnit(thread) {
  return { thread, changeId: null, end: null, events: [] };
}

// When a standing Apply covered the thread: from its arming to its drop or fire.
function armedIntervals(stream) {
  const intervals = [];
  let since = null;
  for (const e of stream) {
    if (e.type === 'StandingApplyArmed' && since == null) since = e.seq;
    if ((e.type === 'StandingApplyDropped' || e.type === 'StandingApplyFired') && since != null) {
      intervals.push([since, e.seq]);
      since = null;
    }
  }
  if (since != null) intervals.push([since, Infinity]);
  return intervals;
}

function deriveUnit(unit, { map, now, armed, global, landings }) {
  const order = map.stages.map((s) => s.name);
  const expected = new Set(map.stages.filter((s) => s.expected).map((s) => s.name));
  const visits = [];
  const unknownStages = [];
  const conditions = [];
  const state = { planKind: null, planBranch: null };
  let stage = null;
  let applyWait = null;
  const open = { questions: new Map(), permissions: new Map(), plan: null, held: null };

  // Waiting on you to Apply runs from Ready to land until the change leaves
  // it or you reply, while no standing Apply covers the thread.
  const openApplyWait = (at) => {
    if (applyWait) return;
    applyWait = { kind: 'apply', start: at, end: null };
    conditions.push(applyWait);
  };
  const closeApplyWait = (at) => {
    if (applyWait) applyWait.end = at;
    applyWait = null;
  };
  const enter = (next, event) => {
    if (next == null || next === stage) return;
    const at = toMs(event.at);
    const from = stage == null ? -1 : order.indexOf(stage);
    const to = order.indexOf(next);
    if (to < 0) throw new Error(`stage map: rule names unknown stage '${next}'`);
    // A plan-only change implements nothing, so it passes no expected stage.
    const planOnly = event.data.content === 'plan-only';
    const skipped = to > from && !planOnly ? order.slice(from + 1, to).filter((s) => expected.has(s)) : [];
    for (const s of skipped) if (!unknownStages.includes(s)) unknownStages.push(s);
    const last = visits[visits.length - 1];
    if (last) {
      last.end = skipped.length ? null : at;
      last.endUnknown = skipped.length > 0;
    }
    visits.push({ stage: next, start: at, end: null });
    if (stage === 'ready-to-land') closeApplyWait(at);
    stage = next;
    if (next === 'ready-to-land' && !covered(armed, event.seq)) openApplyWait(at);
  };

  for (const event of unit.events) {
    const d = event.data;
    if (PLAN_FACTS.has(event.type) && d.kind) state.planKind = d.kind;
    if (event.type === 'PlanApproved') state.planBranch = d.branch_name ?? null;
    const at = toMs(event.at);
    if ((event.type === 'MessageReceived' && d.mode === 'human') || event.type === 'StandingApplyArmed') closeApplyWait(at);
    if (event.type === 'StandingApplyDropped' && stage === 'ready-to-land') openApplyWait(at);
    trackConditions(event, at, open, conditions);
    enter(resolve(map.rules, event, stage, state), event);
    // Later evidence on an ended change, such as a revert, moves no stage.
    if (TERMINALS.has(event.type) && d.change_id === unit.changeId) break;
  }

  const applied = unit.end === 'applied' ? unit.events.findLast((e) => e.type === 'ChangeApplied') : null;
  const content = applied ? applied.data.content : unit.events.findLast((e) => e.type === 'ChangeProposed')?.data.content;
  const rollsOut = applied != null && content !== 'plan-only';
  const live = rollsOut ? findLive(applied, global, landings) : null;
  if (live) {
    if (live.switchWait) conditions.push(live.switchWait);
    // With nothing to roll out, the change goes live as it lands.
    if (live.atLanding) visits.pop();
    else visits[visits.length - 1].end = live.at;
    visits.push({ stage: 'live', start: live.at, end: null });
  }
  // A change still on its way is censored at observation time. A finished,
  // discarded or superseded change's open intervals have no evidenced end.
  const onItsWay = unit.end == null || (rollsOut && !live);
  if (onItsWay) {
    for (const c of conditions) if (c.end == null) Object.assign(c, { end: now, censored: true });
    const last = visits[visits.length - 1];
    if (last && last.end == null && !last.endUnknown) Object.assign(last, { end: now, censored: true });
  }

  const firstPlan = unit.events.find((e) => PLAN_FACTS.has(e.type));
  return {
    thread: unit.thread,
    changeId: unit.changeId,
    end: unit.end,
    content: content ?? null,
    planKind: state.planKind,
    planBranch: state.planBranch,
    events: unit.events,
    visits,
    unknownStages,
    conditions,
    firstPlanFact: firstPlan ? toMs(firstPlan.at) : null,
    landedAt: applied ? toMs(applied.at) : null,
    liveAt: live ? live.at : null,
    leadTime: leadTime({ firstPlan, applied, content, live, conditions }),
  };
}

function resolve(rules, event, stage, unit) {
  for (const rule of rules) {
    if (rule.on !== event.type) continue;
    if (rule.from && !rule.from.includes(stage)) continue;
    if (rule.when && !rule.when(event.data, unit)) continue;
    return typeof rule.to === 'function' ? rule.to(event.data, unit) : rule.to;
  }
  return null;
}

function trackConditions(event, at, open, conditions) {
  const d = event.data;
  const raise = (kind) => {
    const c = { kind, start: at, end: null };
    conditions.push(c);
    return c;
  };
  switch (event.type) {
    case 'UserQuestionAsked':
      open.questions.set(d.tool_use_id, raise(d.permission_request ? 'permission' : 'question'));
      break;
    case 'UserQuestionAnswered':
      settle(open.questions, d.tool_use_id, at);
      break;
    case 'CodingAgentPermissionRequest':
      open.permissions.set(d.request_id, raise('permission'));
      break;
    case 'CodingAgentPermissionResolved':
      settle(open.permissions, d.request_id, at);
      break;
    case 'PlanProposed':
      if (open.plan) open.plan.end = at;
      open.plan = raise('plan-approval');
      break;
    case 'PlanApproved':
      if (d.kind === 'simple-fix' || d.kind === 'bounded-security-fix') break;
      if (open.plan) open.plan.end = at;
      open.plan = null;
      break;
    case 'ProposalWithheld':
      if (!open.held) open.held = raise('held');
      break;
    case 'ChangeProposed':
      if (open.held) open.held.end = at;
      open.held = null;
      break;
  }
}

function settle(map, key, at) {
  const c = map.get(key);
  if (c) c.end = at;
  map.delete(key);
}

function covered(intervals, seq) {
  return intervals.some(([from, to]) => from <= seq && seq <= to);
}

// Live: the running build contains the change's merge commit, or a later
// landing's. Containment is a join on landings in event order, never a Git
// call. A change with nothing to rebuild or refresh is live once landed.
function findLive(applied, global, landings) {
  const d = applied.data;
  if (!d.requires_restart && !d.client_update) return { at: toMs(applied.at), atLanding: true };
  const later = landings.filter((e) => e.seq >= applied.seq);
  const shas = later.flatMap((e) => [e.data.post_merge_sha, e.data.landing_merge_sha]).filter(Boolean);
  const contains = (commit) => commitIn(commit, shas);
  const after = global.filter((e) => e.seq > applied.seq);
  if (d.requires_restart) {
    const activation = after.find((e) => e.type === 'EngineVersionActivated' && contains(engineCommit(e.data.build_id)));
    if (!activation) return null;
    const built = after.find(
      (e) => e.seq < activation.seq && e.type === 'EngineBuildCompleted' && e.data.outcome === 'succeeded' && contains(e.data.built_head),
    );
    const at = toMs(activation.at);
    return { at, switchWait: { kind: 'switch', start: built ? toMs(built.at) : null, end: at } };
  }
  const served = after.find(
    (e) =>
      (e.type === 'ServedFrontendCommitChanged' && contains(e.data.source_commit)) ||
      (e.type === 'EngineVersionActivated' && contains(e.data.frontend_source_commit)),
  );
  return served ? { at: toMs(served.at) } : null;
}

// An engine build id is a short commit, with a hash suffix for a dirty tree.
function engineCommit(buildId) {
  return typeof buildId === 'string' ? buildId.split('-')[0] : null;
}

function commitIn(commit, shas) {
  if (typeof commit !== 'string' || commit.length < 7) return false;
  return shas.some((sha) => sha.startsWith(commit) || commit.startsWith(sha));
}

// First plan fact to Live, minus the union of the waits on the user. Unknown
// when an endpoint or a wait boundary has no evidence.
function leadTime({ firstPlan, applied, content, live, conditions }) {
  if (applied && content === 'plan-only') return { unknown: 'plan-only' };
  if (!firstPlan) return { unknown: 'no-plan-fact' };
  if (!live) return { unknown: 'not-live' };
  const start = toMs(firstPlan.at);
  const end = live.at;
  const waits = conditions.filter((c) => USER_WAITS.has(c.kind));
  const clipped = [];
  for (const w of waits) {
    if (w.start != null && (w.start >= end || (w.end != null && w.end <= start))) continue;
    if (w.start == null || w.end == null) return { unknown: `unclosed-${w.kind}` };
    clipped.push([Math.max(w.start, start), Math.min(w.end, end)]);
  }
  const waitMs = unionLength(clipped);
  return { ms: end - start - waitMs, waitMs, spanMs: end - start };
}

export function unionLength(intervals) {
  const sorted = intervals.filter(([a, b]) => b > a).sort((x, y) => x[0] - y[0]);
  let total = 0;
  let [curStart, curEnd] = [null, null];
  for (const [a, b] of sorted) {
    if (curEnd == null || a > curEnd) {
      if (curEnd != null) total += curEnd - curStart;
      [curStart, curEnd] = [a, b];
    } else if (b > curEnd) {
      curEnd = b;
    }
  }
  if (curEnd != null) total += curEnd - curStart;
  return total;
}

function toMs(at) {
  return typeof at === 'number' ? at : Date.parse(at);
}
