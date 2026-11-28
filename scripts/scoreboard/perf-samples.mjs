import { spawnSync } from 'node:child_process';
import { summarize } from './unit-costs.mjs';

export const DIAGNOSTIC_WINDOW_DAYS = 7;
export const PAGE_LIMIT = 2000;
export const PAGE_BYTE_LIMIT = 262_144;
export const MAX_PAGES = 25;

const DAY_MS = 86_400_000;
const TAG = '[Client/perf]';

export const PERF_VALUE_FIELD = {
  interaction: 'durationMs',
  'main-thread-stall': 'overshootMs',
  loaf: 'durationMs',
  longtask: 'durationMs',
  'thread-load': 'fetchMs',
  'thread-backfill': 'fetchMs',
  'thread-render': 'renderMs',
  'thread-rerender': 'rerenderMs',
  group: 'groupMs',
  navigation: 'ms',
  boot: 'splashLiftedMs',
};

export function deviceClass(ua) {
  if (/iPad|Tablet/i.test(ua)) return 'tablet';
  if (/iPhone|iPod|Android.*Mobile|Mobile.*Safari/i.test(ua)) return 'phone';
  if (/Android/i.test(ua)) return 'tablet';
  if (!ua) return 'unknown';
  return 'desktop';
}

export function parsePerfLine(msg) {
  if (typeof msg !== 'string' || !msg.startsWith(`${TAG} `)) return null;
  const match = /^\[Client\/perf\] (\S+) (\{.*\})(?: code=\S+)?(?: ua=(.*))?$/.exec(msg);
  if (!match) return null;
  const field = PERF_VALUE_FIELD[match[1]];
  if (!field) return null;
  let data;
  try {
    data = JSON.parse(match[2]);
  } catch {
    return null;
  }
  const value = data[field];
  if (!Number.isFinite(value)) return null;
  return { kind: match[1], device: deviceClass(match[3] ?? ''), value };
}

export function perfRollup(samples) {
  const groups = new Map();
  for (const s of samples) {
    const key = `${s.kind}\u0000${s.device}`;
    if (!groups.has(key)) groups.set(key, { kind: s.kind, device: s.device, values: [] });
    groups.get(key).values.push(s.value);
  }
  return [...groups.values()]
    .map(({ kind, device, values }) => ({ kind, device, ...summarize(values) }))
    .sort((a, b) => a.kind.localeCompare(b.kind) || a.device.localeCompare(b.device));
}

export function runDiagnosticQuery(args) {
  const run = spawnSync('lucidos', ['diagnostic-log', 'query', ...args, '--json'], { encoding: 'utf8', maxBuffer: 1 << 26 });
  if (run.error) return { error: run.error.code === 'ENOENT' ? 'the lucidos command is not on PATH' : run.error.message };
  if (run.status !== 0) return { error: (run.stderr || run.stdout || `exit ${run.status}`).trim().split('\n')[0] };
  try {
    return { response: JSON.parse(run.stdout) };
  } catch {
    return { error: 'the diagnostic log answered with text that is not JSON' };
  }
}

export function collectPerfSamples({ days, observedMs, query = runDiagnosticQuery }) {
  const notes = [];
  const windowDays = Math.min(days, DIAGNOSTIC_WINDOW_DAYS);
  if (days > DIAGNOSTIC_WINDOW_DAYS) notes.push(`the diagnostic log answers at most ${DIAGNOSTIC_WINDOW_DAYS} days; showing the last ${DIAGNOSTIC_WINDOW_DAYS}`);
  const since = new Date(observedMs - windowDays * DAY_MS).toISOString();
  const samples = [];
  let lineCount = 0;
  let boundaryCounts = new Map();
  let until = new Date(observedMs).toISOString();
  for (let page = 1; ; page++) {
    const { response, error } = query(['--since', since, '--until', until, '--contains', TAG, '--limit', String(PAGE_LIMIT), '--byte-limit', String(PAGE_BYTE_LIMIT)]);
    if (error) {
      if (page === 1) return { unavailable: error, samples, notes };
      notes.push(`stopped after ${page - 1} pages: ${error}`);
      break;
    }
    if (page === 1) notes.push(...sourceNotes(response.sources), ...coverageNotes(response.coverage_starts_at, since));
    const lines = response.lines ?? [];
    let added = 0;
    for (const line of lines) {
      const key = `${line.pid}|${line.msg}`;
      const alreadyCounted = line.ts === until ? boundaryCounts.get(key) ?? 0 : 0;
      if (alreadyCounted > 0) {
        boundaryCounts.set(key, alreadyCounted - 1);
        continue;
      }
      added++;
      const sample = parsePerfLine(line.msg);
      if (sample) samples.push(sample);
    }
    lineCount += added;
    const more = response.truncated || response.earlier_matches > 0;
    if (!more) break;
    if (!lines.length || !added) {
      notes.push(`older samples are left out: the log returned no new lines (${response.earlier_matches ?? 'some'} earlier matches)`);
      break;
    }
    if (page === MAX_PAGES) {
      notes.push(`older samples are left out: stopped after ${MAX_PAGES} pages of ${PAGE_LIMIT} lines (${response.earlier_matches ?? 'some'} earlier matches)`);
      break;
    }
    until = lines[0].ts;
    boundaryCounts = new Map();
    for (const line of lines) {
      if (line.ts !== until) continue;
      const key = `${line.pid}|${line.msg}`;
      boundaryCounts.set(key, (boundaryCounts.get(key) ?? 0) + 1);
    }
  }
  if (!lineCount) notes.push('no perf samples in the window');
  if (samples.some((s) => s.device === 'desktop')) notes.push('desktop includes iPads on default Safari settings, which send a Mac user agent');
  return { samples, notes };
}

function coverageNotes(coverageStartsAt, since) {
  if (!coverageStartsAt || !(Date.parse(coverageStartsAt) > Date.parse(since))) return [];
  return [`partial history: the diagnostic log keeps lines only from ${coverageStartsAt}`];
}

function sourceNotes(sources) {
  return Object.entries(sources ?? {})
    .filter(([, state]) => state !== 'ok')
    .map(([source, state]) => `${source} log source: ${state}`);
}

export function perfSamplesSection({ days, observedMs, query }) {
  const { samples, notes, unavailable } = collectPerfSamples({ days, observedMs, query });
  if (unavailable) return { unavailable, notes };
  return { samples: samples.length, rollup: perfRollup(samples), notes };
}
