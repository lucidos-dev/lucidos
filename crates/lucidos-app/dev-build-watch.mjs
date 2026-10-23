// Fresh-build-per-change dev frontend watcher (the checkout-level build-watch).
//
// Replaces `vite build --watch`. That command keeps ONE long-lived Rollup
// incremental cache alive for the life of the process, and a watch that survives
// many engine-only Apply restarts (it does — see `.claude/rules/dev-runtime.md` § "Frontend: the engine serves the built `dist/`") can
// WEDGE that cache: it re-emits fresh JS from changed source while serving a
// FROZEN, stale CSS bundle. The engine then serves styles that no longer match
// the source — silently, for hours, with no health check able to see it. (This
// is exactly the "I applied a CSS change and the screen never updated" failure.)
//
// Instead we run a CLEAN `vite build` in a fresh child process on every change.
// A fresh process has NO incremental cache to corrupt: it re-reads all source
// from disk and runs the full plugin chain (atomic publish, sw-stamp, public
// sync, build-id stamp), so the served dist/ can never drift from source. Builds
// here are sub-second, so a full build per change costs nothing noticeable — and
// the entire class of stale-CSS wedges is gone, with no watchdog, no age-based
// recycle, and no staleness guard needed.
//
// Lifecycle: workspace.sh:start_frontend_built launches this through
// scripts/lib/build_watch_exec.pl, which hands it `.build-watch/watch.lock` to
// hold for its whole life and publishes its PID as `.build-watch/pid`. The
// launcher waits for the initial build to produce dist/index.html, and the
// teardown SIGTERMs it. A change mid-build is coalesced and rebuilt after.
//
// # Three things this does besides building
//
// **It installs what the manifest declares.** A coding agent's Apply can land a
// new `package-lock.json` that the checkout never installed: `ensure_npm_deps`
// refuses to install while this watcher runs, by design. Every build after
// that fails to resolve the new import, and `dist/` stops publishing for every
// workspace. So each build first reconciles `node_modules` with the lockfile.
//
// **It says so when a build fails.** The atomic publish keeps the previous
// `dist/` rather than shipping a broken one, which is right and is also what
// makes a failure invisible. A failing build now writes `.build-watch/status.json`
// and, once the failure outlives a short grace period, raises one notification,
// so nobody discovers it hours later.
//
// Both, and why: `docs/plans/2026-08-21-a-wedged-frontend-build-heals-itself-and-shouts.md`.
//
// **It serves a build over a chunk budget, and says so.** Every other build
// fails on that (`vite/chunkBudget.ts`). Here it would strand every later
// Apply, so the child is told it runs under the watcher, and the overrun lands
// in the status file and one notification per budget instead (ADR 0353,
// ADR 0436).

import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, watch } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP_DIR = dirname(fileURLToPath(import.meta.url));
const PROJECT_DIR = resolve(APP_DIR, '../..');
const DEPS_STATE = resolve(PROJECT_DIR, 'scripts/deps-state.sh');
const STATE_DIR = resolve(APP_DIR, '.build-watch');
const STATUS_FILE = resolve(STATE_DIR, 'status.json');
const DEBOUNCE_MS = 200; // coalesce git-merge bursts (Apply touches many files at once)

// Both mirror `vite/chunkBudget.ts`, which this JS cannot import.
// `dev-build-watch.test.ts` fails when they drift apart.
export const DEV_BUILD_WATCH_ENV = 'LUCIDOS_DEV_BUILD_WATCH';
export const BUDGET_LINE_PREFIX = 'lucidos-chunk-budget ';

/** Build output kept for the status file and the alert. Enough for a Rollup
 *  resolve error with its stack, small enough to hold for every build.
 *
 *  Bytes rather than lines, because the output arrives in chunks that split
 *  wherever the pipe decides. Slicing a line array per chunk would keep those
 *  fragments as if they were lines, and `firstErrorLine` would then answer with
 *  half of one. */
const ERROR_TAIL_BYTES = 8000;

// A clean build re-reads everything, so we only need to know "did anything
// change", not what. Watch the bundle's inputs: the app source tree, public/
// (sw.js / manifest / icons), the Vite plugins vite.config.ts imports, the two
// root files that feed the build, and the SDK source (aliased into the bundle
// as @lucidos/sdk).
// The engine mirrors both lists in `files_have_client_update`
// (crates/lucidos-engine/src/engine/git_ops/restart_detection.rs) to decide
// whether an Apply waits for this watch to rebuild. Change them together.
const watchDirs = [
  resolve(APP_DIR, 'src'),
  resolve(APP_DIR, 'public'),
  resolve(APP_DIR, 'vite'),
  resolve(APP_DIR, '../../packages/lucidos-sdk/src'),
];
// The dependency manifests are watched too, and they are not bundle inputs.
// `ensureDeps` runs per build, so without these a dependency-ONLY Apply fires
// no build, installs nothing, and leaves `dist/` on the old tree until some
// unrelated source edit happens along. A lockfile bump that FIXES a broken
// build would never take effect on its own.
const watchFiles = [
  resolve(APP_DIR, 'index.html'),
  resolve(APP_DIR, 'vite.config.ts'),
  resolve(APP_DIR, 'package.json'),
  resolve(PROJECT_DIR, 'package.json'),
  resolve(PROJECT_DIR, 'package-lock.json'),
];

// ── pure helpers, exported for tests ────────────────────────────────────────

/**
 * Should a build outcome be announced, and as what?
 *
 * `prevOk` is `null` before this process has built anything. A first build that
 * fails is news; a first build that succeeds is not, so only the failing
 * direction speaks from the unknown state.
 *
 * A build fires on every keystroke-sized change, so announcing each failure
 * would be a notification storm. Only the edges speak.
 */
export function alertTransition(prevOk, nextOk) {
  if (prevOk === null) return nextOk ? null : 'broken';
  if (prevOk === nextOk) return null;
  return nextOk ? 'recovered' : 'broken';
}

/** How long a failing build is held back before it is announced. An Apply's
 *  merge can catch a build half-written, and the next build heals it. */
export const BROKEN_ALERT_GRACE_MS = 30_000;

/**
 * The build-outcome alerts, with the failing one held for a grace period.
 *
 * A failure healed inside the grace period raises nothing at all, neither the
 * failure nor its recovery. A recovery speaks only after an announced failure.
 * The status file is written regardless, so the engine still sees every build.
 *
 * A grace period that ends mid-build waits for another one. That build may be
 * the fix, and an `npm ci` inside it can outlast the grace period on its own.
 */
export function createBuildAlerts({ raise, setTimer, clearTimer, buildRunning, graceMs = BROKEN_ALERT_GRACE_MS }) {
  let lastOk = null;
  let latestError = null;
  let held = null;
  const announceBroken = () => {
    if (buildRunning()) {
      held = setTimer(announceBroken, graceMs);
      return;
    }
    held = null;
    raise({
      title: 'Frontend build is failing',
      message: `Nothing new is being served from this checkout until it builds. ${latestError ?? ''}`,
    });
  };
  return {
    record(ok, error) {
      const transition = alertTransition(lastOk, ok);
      lastOk = ok;
      if (!ok) latestError = error;
      if (transition === 'broken') {
        held = setTimer(announceBroken, graceMs);
      } else if (transition === 'recovered') {
        if (held !== null) {
          clearTimer(held);
          held = null;
        } else {
          raise({ title: 'Frontend build is green again', message: 'The checkout is publishing again.' });
        }
      }
    },
  };
}

/**
 * The first line of build output worth showing a human.
 *
 * Rollup puts the useful sentence on the line after `error during build:`, and
 * Vite's own failures start with a cross. Falling back to the last non-empty
 * line beats an empty message, which tells the reader nothing at all.
 */
export function firstErrorLine(output) {
  const lines = output.split('\n').map((l) => l.trimEnd()).filter((l) => l.trim() !== '');
  const marker = lines.findIndex((l) => l.startsWith('error during build:'));
  if (marker !== -1 && lines[marker + 1]) return lines[marker + 1].trim();
  const cross = lines.find((l) => l.startsWith('✗'));
  if (cross) return cross.trim();
  return lines.length ? lines[lines.length - 1].trim() : 'the build failed with no output';
}

/**
 * The record written to `.build-watch/status.json` after every build.
 *
 * Written on success too. The engine reads this to explain an Apply that did
 * not land, and a stale failure from an hour ago would be a lie.
 */
export function buildStatusRecord({ ok, at, error, skippedInstall, chunkBudgets }) {
  return {
    ok,
    at,
    error: ok ? null : (error ?? null),
    skippedInstall: skippedInstall ?? null,
    chunkBudgets: chunkBudgets ?? {},
  };
}

/**
 * The environment of one `vite build` child.
 *
 * `LUCIDOS_ATOMIC_DIST` stages the build and publishes it onto `dist/` only on
 * success. `DEV_BUILD_WATCH_ENV` names this process as the parent, which is the
 * only form the budget plugin accepts as "report, do not fail".
 */
export function buildChildEnv(baseEnv, watcherPid) {
  return { ...baseEnv, LUCIDOS_ATOMIC_DIST: '1', [DEV_BUILD_WATCH_ENV]: String(watcherPid) };
}

/**
 * One budget's measurement on one line of build output, or `null`.
 *
 * `overBudget` is decided here rather than trusted from the line, so the status
 * file cannot disagree with its own two numbers.
 */
export function budgetFromLine(line) {
  if (!line.startsWith(BUDGET_LINE_PREFIX)) return null;
  try {
    const { budget, label, subject, bytes, budgetBytes } = JSON.parse(line.slice(BUDGET_LINE_PREFIX.length));
    if (typeof budget !== 'string' || typeof label !== 'string' || typeof subject !== 'string') return null;
    if (!Number.isFinite(bytes) || !Number.isFinite(budgetBytes)) return null;
    return { budget, label, subject, bytes, budgetBytes, overBudget: bytes > budgetBytes };
  } catch {
    return null;
  }
}

/** The notification for one budget crossing its line, either way. */
export function budgetAlert(transition, measured) {
  const kb = (bytes) => `${(bytes / 1000).toFixed(2)} kB`;
  if (transition === 'broken') {
    return {
      title: `${measured.label} is over its budget`,
      message: `${measured.subject} is ${kb(measured.bytes)}, over ${kb(measured.budgetBytes)}. `
        + 'This checkout still serves it, but every other build now fails. See ADR 0436.',
    };
  }
  return {
    title: `${measured.label} is back within its budget`,
    message: `${measured.subject} is ${kb(measured.bytes)}, within ${kb(measured.budgetBytes)}.`,
  };
}

/**
 * The alerts one build's measurements raise, and each budget's state after it.
 *
 * `within` maps a budget name to whether its last measurement was within. Each
 * budget follows `alertTransition` on its own. A budget this build did not
 * measure keeps its state, since a build that failed early is no recovery.
 */
export function budgetAlerts(within, chunkBudgets) {
  const next = { ...within };
  const alerts = [];
  for (const measured of Object.values(chunkBudgets)) {
    const transition = alertTransition(within[measured.budget] ?? null, !measured.overBudget);
    next[measured.budget] = !measured.overBudget;
    if (transition) alerts.push(budgetAlert(transition, measured));
  }
  return { alerts, within: next };
}

// ── the watcher ─────────────────────────────────────────────────────────────

function log(message) {
  console.log(`[dev-build-watch] ${message}`);
}

/** Run `scripts/deps-state.sh <arg>` and hand back its stdout, or `null` when
 *  it could not answer. A missing or broken probe must never stop a build. */
function depsState(arg) {
  try {
    const out = spawnSync('bash', [DEPS_STATE, arg], { encoding: 'utf8' });
    if (out.status !== 0 || typeof out.stdout !== 'string') return null;
    return out.stdout.trim();
  } catch {
    return null;
  }
}

/**
 * Reconcile `node_modules` with the committed lockfile before building.
 *
 * Returns `null` when nothing was needed or the install succeeded, and a reason
 * string when the deps are behind and could not be fixed. That string reaches
 * the status file, so a refused install is visible rather than a mystery.
 *
 * `npm ci`, never `npm install`: ADR 0020. This restores the committed lockfile
 * and must never rewrite it.
 */
function ensureDeps() {
  const want = depsState('fingerprint');
  const stampPath = depsState('stamp-path');
  if (want === null || stampPath === null) return null;

  let have = null;
  try {
    have = readFileSync(stampPath, 'utf8').trim();
  } catch {
    have = null;
  }
  if (have === want) return null;

  const root = depsState('install-root');
  if (root === null) return null;
  log('dependencies changed, running npm ci');
  const rc = spawnSync('npm', ['ci'], { cwd: root, stdio: 'inherit' }).status;
  if (rc !== 0) return `npm ci failed (exit ${rc})`;
  try {
    // Written only after a successful install, so a failed one is retried.
    writeFileSync(stampPath, `${want}\n`);
  } catch {
    // A stamp we cannot write costs one redundant install next time, which is
    // strictly better than skipping the install.
  }
  log('dependencies installed');
  return null;
}

/** Tell the user, once per transition. Best effort by construction: the watcher
 *  publishing the frontend matters more than any alert it can send. */
function raiseAlert({ title, message }) {
  const cli = process.env.LUCIDOS_CLI_BIN;
  const workspace = process.env.LUCIDOS_WORKSPACE;
  if (!cli || !workspace) {
    log(`no alert sent (${!cli ? 'LUCIDOS_CLI_BIN' : 'LUCIDOS_WORKSPACE'} unset)`);
    return;
  }
  try {
    const child = spawn(cli, ['notify', '--title', title, '--message', message], {
      stdio: 'ignore',
      detached: true,
    });
    child.on('error', (err) => log(`alert failed: ${err.message}`));
    child.unref();
  } catch (err) {
    log(`alert failed: ${err.message}`);
  }
}

let building = false;
let pending = false;
let child = null;
const buildAlerts = createBuildAlerts({
  raise: raiseAlert,
  setTimer: setTimeout,
  clearTimer: clearTimeout,
  buildRunning: () => building || pending,
});
/** Per budget, whether its last measurement was within. See `budgetAlerts`. */
let budgetsWithin = {};

function recordOutcome(ok, error, skippedInstall, chunkBudgets) {
  try {
    mkdirSync(STATE_DIR, { recursive: true });
    writeFileSync(
      STATUS_FILE,
      `${JSON.stringify(
        buildStatusRecord({ ok, at: new Date().toISOString(), error, skippedInstall, chunkBudgets }),
        null,
        2,
      )}\n`,
    );
  } catch (err) {
    log(`could not write status: ${err.message}`);
  }
  buildAlerts.record(ok, error);

  for (const measured of Object.values(chunkBudgets)) {
    if (!measured.overBudget) continue;
    log(`${measured.label.toUpperCase()} OVER BUDGET: ${measured.subject} is ${measured.bytes} bytes, `
      + `budget ${measured.budgetBytes}. Served anyway; every other build fails on it.`);
  }
  const { alerts, within } = budgetAlerts(budgetsWithin, chunkBudgets);
  budgetsWithin = within;
  for (const alert of alerts) raiseAlert(alert);
}

/** Vite's CLI, run by this Node directly, so the child's parent is this process. */
function viteBin() {
  const require = createRequire(resolve(APP_DIR, 'package.json'));
  return resolve(dirname(require.resolve('vite/package.json')), 'bin/vite.js');
}

function runBuild() {
  if (building) { pending = true; return; }
  building = true;
  const started = Date.now();
  const skippedInstall = ensureDeps();
  if (skippedInstall) log(skippedInstall);

  let bin;
  try {
    bin = viteBin();
  } catch (err) {
    building = false;
    log(`vite build FAILED: cannot resolve vite (${err.message})`);
    recordOutcome(false, `cannot resolve vite: ${err.message}`, skippedInstall, {});
    return;
  }

  // Fresh child process every time → no incremental cache to wedge.
  //
  // Piped rather than inherited, so the tail can be kept for the status file
  // and the alert. Everything still reaches this process's stdout, which
  // workspace.sh redirects to `.build-watch/log`, so the log is unchanged.
  child = spawn(process.execPath, [bin, 'build'], {
    cwd: APP_DIR,
    env: buildChildEnv(process.env, process.pid),
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let tail = '';
  let partialLine = '';
  const chunkBudgets = {};
  const measure = (line) => {
    const measured = budgetFromLine(line);
    if (measured) chunkBudgets[measured.budget] = measured;
  };
  const keep = (chunk) => {
    process.stdout.write(chunk);
    tail = (tail + chunk.toString()).slice(-ERROR_TAIL_BYTES);
  };
  // Whole lines only: the measurement can arrive split across two chunks.
  const keepStdout = (chunk) => {
    keep(chunk);
    const lines = (partialLine + chunk.toString()).split('\n');
    partialLine = lines.pop() ?? '';
    for (const line of lines) measure(line);
  };
  child.stdout.on('data', keepStdout);
  child.stderr.on('data', keep);

  child.on('exit', (code) => {
    child = null;
    building = false;
    measure(partialLine);
    const ms = Date.now() - started;
    const ok = code === 0;
    log(`vite build ${ok ? 'ok' : `FAILED (exit ${code})`} in ${ms}ms`);
    recordOutcome(ok, ok ? null : firstErrorLine(tail), skippedInstall, chunkBudgets);
    if (pending) { pending = false; runBuild(); }
  });
}

let timer = null;
function schedule() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(runBuild, DEBOUNCE_MS);
}

const watchers = [];

function startWatching() {
  for (const dir of watchDirs) {
    try {
      // Recursive fs.watch is supported on macOS/Windows and Linux (Node 20+);
      // the dev harness only runs on developer machines, all of which qualify.
      watchers.push(watch(dir, { recursive: true }, schedule));
    } catch (err) {
      console.warn(`[dev-build-watch] cannot watch ${dir}: ${err.message}`);
    }
  }
  for (const file of watchFiles) {
    try {
      watchers.push(watch(file, schedule));
    } catch (err) {
      console.warn(`[dev-build-watch] cannot watch ${file}: ${err.message}`);
    }
  }
}

function shutdown() {
  for (const w of watchers) { try { w.close(); } catch { /* already closed */ } }
  if (child) child.kill('SIGTERM');
  process.exit(0);
}

// Only when run as the watcher. Importing this file for its pure helpers must
// not start a build, which is what lets them be unit-tested at all.
const invokedDirectly = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  // Initial build. workspace.sh waits for dist/index.html before starting the engine.
  runBuild();
  startWatching();
  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);
}
