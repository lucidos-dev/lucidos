/**
 * The build-watch's pure helpers: when a build outcome is worth announcing,
 * what to say about a failure, and what goes in the status file.
 *
 * These exist because a wedged frontend build used to be invisible. The atomic
 * publish keeps the previous `dist/`, which is correct and is also why nobody
 * notices. An Apply landed a package the checkout had not installed. Every
 * build then failed for over half an hour, and the only record was a log file.
 * See `docs/plans/2026-08-21-a-wedged-frontend-build-heals-itself-and-shouts.md`.
 *
 * Importing the watcher is safe: it starts a build only when it IS the entry
 * module, which is exactly what makes these testable.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: dev tooling, JS with no type declarations
import * as watcher from '../../dev-build-watch.mjs';
import {
  BUDGET_LINE_PREFIX,
  DEV_BUILD_WATCH_ENV,
  ENTRY_BUDGET,
  FIRST_PAINT_BUDGET,
  budgetLine,
  underDevBuildWatch,
} from '../../vite/chunkBudget';

const {
  BUDGET_LINE_PREFIX: WATCHER_PREFIX,
  DEV_BUILD_WATCH_ENV: WATCHER_ENV,
  BROKEN_ALERT_GRACE_MS,
  alertTransition,
  createBuildAlerts,
  budgetAlert,
  budgetAlerts,
  budgetFromLine,
  buildChildEnv,
  buildStatusRecord,
  firstErrorLine,
} = watcher;

describe('when a build outcome is announced', () => {
  it('speaks on the way into failing, and on the way out', () => {
    expect(alertTransition(true, false)).toBe('broken');
    expect(alertTransition(false, true)).toBe('recovered');
  });

  it('stays quiet while nothing changed', () => {
    // A build fires on every keystroke-sized change. Announcing each failure
    // would be a notification storm, so only the edges speak.
    expect(alertTransition(false, false)).toBeNull();
    expect(alertTransition(true, true)).toBeNull();
  });

  it('announces a first build only when it failed', () => {
    // `null` is the state before this process has built anything. A first build
    // that fails is news. One that succeeds recovered nothing.
    expect(alertTransition(null, false)).toBe('broken');
    expect(alertTransition(null, true)).toBeNull();
  });
});

describe('a failure that heals within the grace period', () => {
  // An Apply's merge can catch a build half-written. The next build is green a
  // second later, and a "failing" then "green again" pair is pure noise.
  function harness() {
    const raised: { title: string; message: string }[] = [];
    const delays: number[] = [];
    const build = { running: false };
    let pendingFire: (() => void) | null = null;
    const alerts = createBuildAlerts({
      raise: (alert: { title: string; message: string }) => raised.push(alert),
      setTimer: (fire: () => void, ms: number) => { pendingFire = fire; delays.push(ms); return 1; },
      clearTimer: () => { pendingFire = null; },
      buildRunning: () => build.running,
    });
    const elapse = () => { const fire = pendingFire; pendingFire = null; fire?.(); };
    return { raised, alerts, elapse, delays, build };
  }

  it('says nothing when the next build is green inside the grace period', () => {
    const { raised, alerts, elapse } = harness();
    alerts.record(true, null);
    alerts.record(false, 'X is not exported');
    alerts.record(true, null);
    elapse();
    expect(raised).toEqual([]);
  });

  it('announces a failure that outlives the grace period, with its latest error', () => {
    const { raised, alerts, elapse } = harness();
    alerts.record(false, 'first error');
    alerts.record(false, 'second error');
    elapse();
    expect(raised).toHaveLength(1);
    expect(raised[0].title).toBe('Frontend build is failing');
    expect(raised[0].message).toContain('second error');
  });

  it('announces the recovery only of a failure it announced', () => {
    const { raised, alerts, elapse } = harness();
    alerts.record(false, 'broken');
    elapse();
    alerts.record(true, null);
    expect(raised.map((a) => a.title)).toEqual([
      'Frontend build is failing',
      'Frontend build is green again',
    ]);
  });

  it('holds the failure for the whole grace period', () => {
    const { alerts, delays } = harness();
    alerts.record(false, 'broken');
    expect(delays).toEqual([BROKEN_ALERT_GRACE_MS]);
  });

  it('waits out a build still running when the grace period ends', () => {
    // A rebuild that runs `npm ci` can outlast the grace period and still heal.
    const { raised, alerts, elapse, delays, build } = harness();
    alerts.record(false, 'broken');
    build.running = true;
    elapse();
    expect(raised).toEqual([]);
    expect(delays).toEqual([BROKEN_ALERT_GRACE_MS, BROKEN_ALERT_GRACE_MS]);
    build.running = false;
    alerts.record(true, null);
    elapse();
    expect(raised).toEqual([]);
  });
});

describe('what a failure says', () => {
  it('takes the line after Rollup names the error', () => {
    const output = [
      'vite v6.4.3 building for production...',
      'transforming...',
      '✗ Build failed in 201ms',
      'error during build:',
      '[vite]: Rollup failed to resolve import "jsqr" from "PairingScanner.tsx".',
      '    at viteLog (file:///node_modules/vite/dist/node/chunks/dep.js:1:1)',
    ].join('\n');
    expect(firstErrorLine(output)).toContain('Rollup failed to resolve import "jsqr"');
  });

  it('falls back to the cross line when there is no Rollup marker', () => {
    expect(firstErrorLine('building...\n✗ Build failed in 12ms\n')).toBe('✗ Build failed in 12ms');
  });

  it('survives a chunk boundary mid-line', () => {
    // The tail is a byte window over piped output, so its first line is
    // routinely half of one. The marker search must still find the real error.
    const truncated = 'ing for production...\ntransforming...\nerror during build:\nthe real error\n';
    expect(firstErrorLine(truncated)).toBe('the real error');
  });

  it('never answers with nothing', () => {
    // An empty message in a notification tells the reader the build broke and
    // nothing else, which is the failure this whole path exists to end.
    expect(firstErrorLine('')).toBeTruthy();
    expect(firstErrorLine('\n\n  \n')).toBeTruthy();
    expect(firstErrorLine('some unrecognised tail')).toBe('some unrecognised tail');
  });
});

/** A measurement as `budgetFromLine` reads it off the plugin's own line. */
function measured(budget: typeof ENTRY_BUDGET, subject: string, bytes: number) {
  return budgetFromLine(budgetLine(budget, { subject, bytes }));
}

const overEntry = measured(ENTRY_BUDGET, 'assets/index-a.js', ENTRY_BUDGET.kb * 1000 + 230);
const withinEntry = measured(ENTRY_BUDGET, 'assets/index-a.js', ENTRY_BUDGET.kb * 800);
const overFirstPaint = measured(FIRST_PAINT_BUDGET, '(2 chunks: a.js, b.js)', FIRST_PAINT_BUDGET.kb * 1000 + 12_340);
const withinFirstPaint = measured(FIRST_PAINT_BUDGET, '(2 chunks: a.js, b.js)', FIRST_PAINT_BUDGET.kb * 800);

describe('the status record', () => {
  const at = '2026-08-21T12:00:00.000Z';

  it('carries the error only when the build failed', () => {
    // A success that kept a stale error would make the engine report a failure
    // that is over.
    expect(buildStatusRecord({ ok: true, at, error: 'stale', skippedInstall: null })).toEqual({
      ok: true,
      at,
      error: null,
      skippedInstall: null,
      chunkBudgets: {},
    });
  });

  it('keeps a served build green while recording each budget overrun', () => {
    // The overrun is reported, never a failure: `ok: false` would tell the
    // engine nothing new is being served, which is the opposite of true.
    const chunkBudgets = { entry: overEntry, 'first-paint': overFirstPaint };
    const record = buildStatusRecord({ ok: true, at, chunkBudgets });
    expect(record.ok).toBe(true);
    expect(record.chunkBudgets).toEqual(chunkBudgets);
  });

  it('records a refused install alongside the outcome', () => {
    // A build that failed because its dependencies could not be installed has
    // two facts, and the second is the actionable one.
    const record = buildStatusRecord({
      ok: false,
      at,
      error: 'Rollup failed to resolve import "jsqr"',
      skippedInstall: 'npm ci failed (exit 1)',
    });
    expect(record.ok).toBe(false);
    expect(record.error).toContain('jsqr');
    expect(record.skippedInstall).toContain('npm ci failed');
  });

  it('normalises a missing error to null rather than undefined', () => {
    // It is serialised to JSON, and `undefined` would drop the key entirely.
    const record = buildStatusRecord({ ok: false, at });
    expect(record.error).toBeNull();
    expect(JSON.parse(JSON.stringify(record))).toHaveProperty('error');
  });
});

describe('the vite build child', () => {
  it('is told it runs under the watcher, bound to the watcher\'s own pid', () => {
    const env = buildChildEnv({ PATH: '/usr/bin' }, 4242);
    expect(env[WATCHER_ENV]).toBe('4242');
    expect(env.LUCIDOS_ATOMIC_DIST).toBe('1');
    expect(env.PATH).toBe('/usr/bin');
    // The child Vite is spawned directly, so its parent IS the watcher, and the
    // budget plugin accepts the signal only in exactly that case.
    expect(underDevBuildWatch(env, 4242)).toBe(true);
  });

  it('overrides a stale value inherited from whatever started the watcher', () => {
    expect(buildChildEnv({ [WATCHER_ENV]: '1' }, 4242)[WATCHER_ENV]).toBe('4242');
  });

  it('shares its two names with the budget plugin', () => {
    expect(WATCHER_ENV).toBe(DEV_BUILD_WATCH_ENV);
    expect(WATCHER_PREFIX).toBe(BUDGET_LINE_PREFIX);
  });
});

describe('reading a budget measurement', () => {
  it('parses the line the budget plugin prints', () => {
    expect(overEntry).toEqual({
      budget: ENTRY_BUDGET.name,
      label: ENTRY_BUDGET.label,
      subject: 'assets/index-a.js',
      bytes: ENTRY_BUDGET.kb * 1000 + 230,
      budgetBytes: ENTRY_BUDGET.kb * 1000,
      overBudget: true,
    });
    expect(withinEntry?.overBudget).toBe(false);
    expect(overFirstPaint?.budget).toBe(FIRST_PAINT_BUDGET.name);
  });

  it('ignores every other line, and a malformed measurement', () => {
    expect(budgetFromLine('dist/assets/index-a.js  600.23 kB')).toBeNull();
    expect(budgetFromLine(`${WATCHER_PREFIX}{not json`)).toBeNull();
    expect(budgetFromLine(`${WATCHER_PREFIX}{"subject":"a.js","bytes":1,"budgetBytes":2}`)).toBeNull();
  });

  it('names the budget, the size and the limit in the overrun alert', () => {
    const alert = budgetAlert('broken', overFirstPaint);
    expect(alert.title).toBe('First-paint set is over its budget');
    expect(alert.message).toContain(`${FIRST_PAINT_BUDGET.kb + 12.34} kB`);
    expect(alert.message).toContain(`${FIRST_PAINT_BUDGET.kb}.00 kB`);
    expect(alert.message).toContain('(2 chunks: a.js, b.js)');
    expect(budgetAlert('recovered', withinEntry).title).toBe('Entry chunk is back within its budget');
  });
});

describe('announcing a budget edge', () => {
  it('stays quiet on a first build within every budget', () => {
    const { alerts } = budgetAlerts({}, { entry: withinEntry, 'first-paint': withinFirstPaint });
    expect(alerts).toEqual([]);
  });

  it('speaks once for the budget that crossed, and only that one', () => {
    const first = budgetAlerts({}, { entry: withinEntry, 'first-paint': withinFirstPaint });
    const second = budgetAlerts(first.within, { entry: withinEntry, 'first-paint': overFirstPaint });
    expect(second.alerts.map((a: { title: string }) => a.title)).toEqual(['First-paint set is over its budget']);
    const third = budgetAlerts(second.within, { entry: withinEntry, 'first-paint': overFirstPaint });
    expect(third.alerts).toEqual([]);
    const fourth = budgetAlerts(third.within, { entry: withinEntry, 'first-paint': withinFirstPaint });
    expect(fourth.alerts.map((a: { title: string }) => a.title)).toEqual(['First-paint set is back within its budget']);
  });

  it('keeps the last state of a budget a build did not measure', () => {
    // A build that failed early prints no measurement. That is not a recovery.
    const over = budgetAlerts({}, { entry: overEntry });
    const silent = budgetAlerts(over.within, {});
    expect(silent.alerts).toEqual([]);
    expect(silent.within).toEqual(over.within);
  });
});
