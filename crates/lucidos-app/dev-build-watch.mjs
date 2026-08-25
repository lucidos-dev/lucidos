// Fresh-build-per-change dev frontend watcher (the checkout-level build-watch).
//
// Replaces `vite build --watch`. That command keeps ONE long-lived Rollup
// incremental cache alive for the life of the process, and a watch that survives
// many engine-only Apply restarts (it does — see `.claude/rules/dev-runtime.md`) can
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
// Lifecycle: workspace.sh:start_frontend_built launches this as the
// checkout-level build-watch singleton (this process's PID is the
// `.build-watch/pid`), waits for the initial build to produce dist/index.html,
// and SIGTERMs it on teardown. A change mid-build is coalesced and rebuilt after.
//
// # Two things this does besides building
//
// **It installs what the manifest declares.** A coding agent's Apply can land a
// new `package-lock.json` that the checkout never installed: `ensure_npm_deps`
// refuses to install while a frontend is running, by design. Every build after
// that fails to resolve the new import, and `dist/` stops publishing for every
// workspace. So each build first reconciles `node_modules` with the lockfile.
//
// **It says so when a build fails.** The atomic publish keeps the previous
// `dist/` rather than shipping a broken one, which is right and is also what
// makes a failure invisible. A failing build now writes `.build-watch/status.json`
// and raises one notification, so nobody discovers it hours later.
//
// Both, and why: `docs/plans/2026-08-21-a-wedged-frontend-build-heals-itself-and-shouts.md`.

import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, watch } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP_DIR = dirname(fileURLToPath(import.meta.url));
const PROJECT_DIR = resolve(APP_DIR, '../..');
const DEPS_STATE = resolve(PROJECT_DIR, 'scripts/deps-state.sh');
const STATE_DIR = resolve(APP_DIR, '.build-watch');
const STATUS_FILE = resolve(STATE_DIR, 'status.json');
const DEBOUNCE_MS = 200; // coalesce git-merge bursts (Apply touches many files at once)

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
// (sw.js / manifest / icons), the two root files that feed the build, and the
// SDK source (aliased into the bundle as @lucidos/sdk).
const watchDirs = [
  resolve(APP_DIR, 'src'),
  resolve(APP_DIR, 'public'),
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
export function buildStatusRecord({ ok, at, error, skippedInstall }) {
  return {
    ok,
    at,
    error: ok ? null : (error ?? null),
    skippedInstall: skippedInstall ?? null,
  };
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

/** True when a Vite dev server in this checkout holds `node_modules`. The probe
 *  exits 0 for a conflict, and excludes this watcher's own pid. */
function devServerRunning() {
  try {
    return spawnSync('bash', [DEPS_STATE, 'dev-server-running']).status === 0;
  } catch {
    return false;
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

  if (devServerRunning()) {
    // The case `ensure_npm_deps` refuses for: wiping node_modules under a live
    // Vite server corrupts it. Say so instead, and build anyway. The build may
    // well fail, and then the alert carries both facts.
    return 'dependencies changed, but a Vite dev server in this checkout holds node_modules';
  }

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
function raiseAlert(kind, detail) {
  const cli = process.env.LUCIDOS_CLI_BIN;
  const workspace = process.env.LUCIDOS_WORKSPACE;
  if (!cli || !workspace) {
    log(`no alert sent (${!cli ? 'LUCIDOS_CLI_BIN' : 'LUCIDOS_WORKSPACE'} unset)`);
    return;
  }
  const title = kind === 'broken' ? 'Frontend build is failing' : 'Frontend build is green again';
  const message = kind === 'broken'
    ? `Nothing new is being served from this checkout until it builds. ${detail}`
    : 'The checkout is publishing again.';
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
/** `null` until this process has completed a build. See `alertTransition`. */
let lastOk = null;

function recordOutcome(ok, error, skippedInstall) {
  try {
    mkdirSync(STATE_DIR, { recursive: true });
    writeFileSync(
      STATUS_FILE,
      `${JSON.stringify(
        buildStatusRecord({ ok, at: new Date().toISOString(), error, skippedInstall }),
        null,
        2,
      )}\n`,
    );
  } catch (err) {
    log(`could not write status: ${err.message}`);
  }
  const transition = alertTransition(lastOk, ok);
  lastOk = ok;
  if (transition) raiseAlert(transition, error ?? '');
}

function runBuild() {
  if (building) { pending = true; return; }
  building = true;
  const started = Date.now();
  const skippedInstall = ensureDeps();
  if (skippedInstall) log(skippedInstall);

  // Fresh child process every time → no incremental cache to wedge.
  // LUCIDOS_ATOMIC_DIST makes the build stage into dist.staging and atomically
  // publish onto dist/ only on success, so a failed build never clobbers dist/.
  //
  // Piped rather than inherited, so the tail can be kept for the status file
  // and the alert. Everything still reaches this process's stdout, which
  // workspace.sh redirects to `.build-watch/log`, so the log is unchanged.
  child = spawn('npx', ['vite', 'build'], {
    cwd: APP_DIR,
    env: { ...process.env, LUCIDOS_ATOMIC_DIST: '1' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let tail = '';
  const keep = (chunk) => {
    process.stdout.write(chunk);
    tail = (tail + chunk.toString()).slice(-ERROR_TAIL_BYTES);
  };
  child.stdout.on('data', keep);
  child.stderr.on('data', keep);

  child.on('exit', (code) => {
    child = null;
    building = false;
    const ms = Date.now() - started;
    const ok = code === 0;
    log(`vite build ${ok ? 'ok' : `FAILED (exit ${code})`} in ${ms}ms`);
    recordOutcome(ok, ok ? null : firstErrorLine(tail), skippedInstall);
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
