import { test } from 'node:test';
import assert from 'node:assert/strict';
import { collectPerfSamples, deviceClass, MAX_PAGES, PAGE_LIMIT, parsePerfLine, perfRollup, perfSamplesSection } from './perf-samples.mjs';
import { formatText } from './report.mjs';

const IPHONE = 'Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1';
const MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)';
const IPAD_NOTE = 'desktop includes iPads on default Safari settings, which send a Mac user agent';

function line(kind, data, ua = MAC, ts = '2026-11-24T10:00:00.000Z', pid = 1) {
  return { ts, pid, msg: `[Client/perf] ${kind} ${JSON.stringify(data)} code=${kind} ua=${ua}` };
}

test('device classes come from the user agent', () => {
  assert.equal(deviceClass(IPHONE), 'phone');
  assert.equal(deviceClass('Mozilla/5.0 (iPad; CPU OS 26_0 like Mac OS X)'), 'tablet');
  assert.equal(deviceClass('Mozilla/5.0 (Linux; Android 15; Pixel 9) Chrome/150 Mobile Safari/537.36'), 'phone');
  assert.equal(deviceClass('Mozilla/5.0 (Linux; Android 15; Tab) Chrome/150 Safari/537.36'), 'tablet');
  assert.equal(deviceClass(MAC), 'desktop');
  assert.equal(deviceClass(''), 'unknown');
});

test('a perf line yields its kind, device class and the kind\'s measure', () => {
  assert.deepEqual(parsePerfLine(line('thread-render', { renderMs: 28, markdownMs: 1 }).msg), { kind: 'thread-render', device: 'desktop', value: 28 });
  assert.deepEqual(parsePerfLine(line('interaction', { durationMs: 144 }, IPHONE).msg), { kind: 'interaction', device: 'phone', value: 144 });
  assert.deepEqual(parsePerfLine('[Client/perf] main-thread-stall {"overshootMs":90}'), { kind: 'main-thread-stall', device: 'unknown', value: 90 });
});

test('lines that are not measures are skipped', () => {
  assert.equal(parsePerfLine('[Client/perf] perf-probe-support {"armed":["event"]} ua=x'), null);
  assert.equal(parsePerfLine('[Client/perf] thread-render {"renderMs":"slow"} ua=x'), null);
  assert.equal(parsePerfLine('[Client/perf] thread-render {not json} ua=x'), null);
  assert.equal(parsePerfLine('[Client/page-health] {"fps":60}'), null);
});

test('the roll-up gives median and 90th percentile per kind and device class', () => {
  const desktop = [10, 20, 30, 40, 50].map((value) => ({ kind: 'thread-render', device: 'desktop', value }));
  const phone = [100, 200, 300, 400, 500, 600, 700, 800, 900, 1000].map((value) => ({ kind: 'thread-render', device: 'phone', value }));
  const stall = [{ kind: 'main-thread-stall', device: 'phone', value: 70 }];
  assert.deepEqual(perfRollup([...phone, ...stall, ...desktop]), [
    { kind: 'main-thread-stall', device: 'phone', n: 1, tooFew: true },
    { kind: 'thread-render', device: 'desktop', n: 5, p50: 30, p90: 46 },
    { kind: 'thread-render', device: 'phone', n: 10, p50: 550, p90: 910 },
  ]);
});

function answers(pages) {
  const asked = [];
  const query = (args) => {
    asked.push(args);
    const next = pages[asked.length - 1];
    return next.error ? next : { response: next };
  };
  return { query, asked };
}

test('older pages are fetched by moving the window end back', () => {
  const newest = { lines: [line('thread-render', { renderMs: 30 }, MAC, '2026-11-24T10:00:02.000Z')], truncated: true, earlier_matches: 1, sources: { engine: 'ok', client: 'ok' } };
  const older = { lines: [line('thread-render', { renderMs: 10 }, MAC, '2026-11-24T10:00:01.000Z')], truncated: false, earlier_matches: 0, sources: { engine: 'ok', client: 'ok' } };
  const { query, asked } = answers([newest, older]);
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.deepEqual(result.samples.map((s) => s.value), [30, 10]);
  assert.deepEqual(result.notes, [IPAD_NOTE]);
  assert.equal(asked.length, 2);
  assert.equal(asked[0][asked[0].indexOf('--until') + 1], '2026-11-24T12:00:00.000Z');
  assert.equal(asked[1][asked[1].indexOf('--until') + 1], '2026-11-24T10:00:02.000Z');
  assert.equal(asked[0][asked[0].indexOf('--since') + 1], '2026-11-23T12:00:00.000Z');
  assert.equal(asked[0][asked[0].indexOf('--limit') + 1], String(PAGE_LIMIT));
});

test('a repeated line across pages counts once', () => {
  const same = line('thread-render', { renderMs: 30 });
  const first = { lines: [same], truncated: true, earlier_matches: 3 };
  const second = { lines: [same], truncated: true, earlier_matches: 2 };
  const { query } = answers([first, second]);
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.equal(result.samples.length, 1);
  assert.match(result.notes.join('\n'), /older samples are left out: the log returned no new lines \(2 earlier matches\)/);
});

test('identical samples from one flush all count', () => {
  const same = line('longtask', { durationMs: 52 }, IPHONE);
  const { query } = answers([{ lines: [same, same, same], truncated: false, earlier_matches: 0 }]);
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.deepEqual(result.samples.map((s) => s.value), [52, 52, 52]);
  assert.deepEqual(result.notes, []);
});

test('the boundary timestamp skips only the lines the newer page already counted', () => {
  const boundary = '2026-11-24T10:00:05.000Z';
  const same = line('longtask', { durationMs: 52 }, IPHONE, boundary);
  const newest = { lines: [same, line('longtask', { durationMs: 80 }, IPHONE, '2026-11-24T10:00:06.000Z')], truncated: true, earlier_matches: 3 };
  const older = { lines: [line('longtask', { durationMs: 10 }, IPHONE, '2026-11-24T10:00:01.000Z'), same, same], truncated: false, earlier_matches: 0 };
  const { query } = answers([newest, older]);
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.deepEqual(result.samples.map((s) => s.value).sort((a, b) => a - b), [10, 52, 52, 80]);
});

test('the page cap is surfaced with the earlier matches it leaves out', () => {
  let n = 0;
  const query = () => ({ response: { lines: [line('thread-render', { renderMs: 5 }, MAC, `2026-11-24T10:00:${String(59 - n++).padStart(2, '0')}.000Z`)], truncated: true, earlier_matches: 900 } });
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.equal(result.samples.length, MAX_PAGES);
  assert.match(result.notes.join('\n'), new RegExp(`stopped after ${MAX_PAGES} pages .*900 earlier matches`));
});

test('a failed first query is unavailable, never an empty roll-up', () => {
  const { query } = answers([{ error: 'the lucidos command is not on PATH' }]);
  assert.deepEqual(perfSamplesSection({ days: 7, observedMs: 0, query }), { unavailable: 'the lucidos command is not on PATH', notes: [] });
});

test('a failure on a later page keeps the samples and says where it stopped', () => {
  const first = { lines: [line('thread-render', { renderMs: 30 })], truncated: true, earlier_matches: 5 };
  const { query } = answers([first, { error: 'timed out' }]);
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.equal(result.samples.length, 1);
  assert.deepEqual(result.notes, ['stopped after 1 pages: timed out', IPAD_NOTE]);
});

test('a source that is not ok and a window past seven days are named', () => {
  const only = { lines: [line('boot', { splashLiftedMs: 800 })], truncated: false, earlier_matches: 0, sources: { engine: 'ok', client: 'team-gateway' } };
  const { query } = answers([only]);
  const result = collectPerfSamples({ days: 30, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.deepEqual(result.notes, ['the diagnostic log answers at most 7 days; showing the last 7', 'client log source: team-gateway', IPAD_NOTE]);
});

test('history the log no longer keeps is named', () => {
  const only = { lines: [line('boot', { splashLiftedMs: 800 }, IPHONE)], truncated: false, earlier_matches: 0, coverage_starts_at: '2026-11-24T08:00:00+00:00' };
  const { query } = answers([only]);
  const result = collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query });
  assert.deepEqual(result.notes, ['partial history: the diagnostic log keeps lines only from 2026-11-24T08:00:00+00:00']);
  const covered = answers([{ ...only, coverage_starts_at: '2026-11-20T08:00:00+00:00' }]);
  assert.deepEqual(collectPerfSamples({ days: 1, observedMs: Date.parse('2026-11-24T12:00:00Z'), query: covered.query }).notes, []);
});

test('an empty window says so', () => {
  const { query } = answers([{ lines: [], truncated: false, earlier_matches: 0, sources: { engine: 'ok' } }]);
  const result = collectPerfSamples({ days: 1, observedMs: 0, query });
  assert.deepEqual(result.notes, ['no perf samples in the window']);
});

test('the text section lists the roll-up and every note', () => {
  const rollup = perfRollup([10, 20, 30, 40, 50].map((value) => ({ kind: 'thread-render', device: 'phone', value })));
  const text = formatText(minimalReport({ samples: 5, rollup, notes: ['client log source: team-gateway'] }));
  assert.match(text, /thread-render\s+phone\s+n=5 p50 30 p90 46/);
  assert.match(text, /note: client log source: team-gateway/);
});

function minimalReport(perf) {
  return {
    days: 7,
    observed_at: '2026-11-24T12:00:00.000Z',
    coverage: { changes: 0, lead_time_known: 0, lead_time_unknown: {} },
    too_few_below: 5,
    lead_time: null,
    waits: null,
    stages: {},
    changes: [],
    classes: {},
    crate_graph: 'unavailable',
    unit_costs: { rounds_per_change: null, changes_without_rounds: 0, failed_phases: {}, round_seconds: { without_start: 0 }, engine_builds: {}, frontend_refresh: null, build_slot: {} },
    rework: { reverted: 0, apply_failed: 0, main_check_failed: 0 },
    harden_suites: { shapes: {}, suites: {} },
    main_check_seconds: {},
    lane_batches: {},
    github_build_seconds: { unavailable: 'no timings log path' },
    perf_samples: perf,
  };
}
