
import { spawn, spawnSync } from 'node:child_process';
import { mkdirSync, readFileSync, writeFileSync, watch } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP_DIR = dirname(fileURLToPath(import.meta.url));
export const PROJECT_DIR = resolve(APP_DIR, '../..');
const DEPS_STATE = resolve(PROJECT_DIR, 'scripts/deps-state.sh');
const STATE_DIR = resolve(APP_DIR, '.build-watch');
const STATUS_FILE = resolve(STATE_DIR, 'status.json');
const DEBOUNCE_MS = 200;
const POLL_MS = 10_000;
const PROBE_TIMEOUT_MS = 5_000;

export const DEV_BUILD_WATCH_ENV = 'LUCIDOS_DEV_BUILD_WATCH';
export const BUDGET_LINE_PREFIX = 'lucidos-chunk-budget ';

const ERROR_TAIL_BYTES = 8000;

export const watchDirs = [
  resolve(APP_DIR, 'src'),
  resolve(APP_DIR, 'public'),
  resolve(APP_DIR, 'vite'),
  resolve(APP_DIR, '../../packages/lucidos-sdk/src'),
];
export const watchFiles = [
  resolve(APP_DIR, 'index.html'),
  resolve(APP_DIR, 'vite.config.ts'),
  resolve(APP_DIR, 'package.json'),
  resolve(PROJECT_DIR, 'package.json'),
  resolve(PROJECT_DIR, 'package-lock.json'),
];


export function alertTransition(prevOk, nextOk) {
  if (prevOk === null) return nextOk ? null : 'broken';
  if (prevOk === nextOk) return null;
  return nextOk ? 'recovered' : 'broken';
}

export const BROKEN_ALERT_GRACE_MS = 30_000;

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

export function firstErrorLine(output) {
  const lines = output.split('\n').map((l) => l.trimEnd()).filter((l) => l.trim() !== '');
  const marker = lines.findIndex((l) => l.startsWith('error during build:'));
  if (marker !== -1 && lines[marker + 1]) return lines[marker + 1].trim();
  const cross = lines.find((l) => l.startsWith('✗'));
  if (cross) return cross.trim();
  return lines.length ? lines[lines.length - 1].trim() : 'the build failed with no output';
}

export function buildStatusRecord({ ok, at, error, skippedInstall, chunkBudgets }) {
  return {
    ok,
    at,
    error: ok ? null : (error ?? null),
    skippedInstall: skippedInstall ?? null,
    chunkBudgets: chunkBudgets ?? {},
  };
}

export function buildChildEnv(baseEnv, watcherPid) {
  return { ...baseEnv, LUCIDOS_ATOMIC_DIST: '1', [DEV_BUILD_WATCH_ENV]: String(watcherPid) };
}

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

export function gitInputPaths(projectDir, absolutePaths) {
  return absolutePaths.map((p) => relative(projectDir, p).split(sep).join('/'));
}

export function pollAction({ built, current, eventSinceBuild }) {
  if (built === null || current === null) return 'none';
  if (built === current || eventSinceBuild) return 'none';
  return 'rearm-and-build';
}


function log(message) {
  console.log(`[dev-build-watch] ${message}`);
}

function depsState(arg) {
  try {
    const out = spawnSync('bash', [DEPS_STATE, arg], { encoding: 'utf8' });
    if (out.status !== 0 || typeof out.stdout !== 'string') return null;
    return out.stdout.trim();
  } catch {
    return null;
  }
}

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
    writeFileSync(stampPath, `${want}\n`);
  } catch {
  }
  log('dependencies installed');
  return null;
}

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

function inputsFingerprint() {
  try {
    const out = spawnSync(
      'git',
      ['ls-tree', 'HEAD', '--', ...gitInputPaths(PROJECT_DIR, [...watchDirs, ...watchFiles])],
      { cwd: PROJECT_DIR, encoding: 'utf8', timeout: PROBE_TIMEOUT_MS },
    );
    if (out.status !== 0 || typeof out.stdout !== 'string' || out.stdout === '') return null;
    return out.stdout;
  } catch {
    return null;
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

function viteBin() {
  const require = createRequire(resolve(APP_DIR, 'package.json'));
  return resolve(dirname(require.resolve('vite/package.json')), 'bin/vite.js');
}

let builtFingerprint = null;
let eventSinceBuild = false;

function runBuild() {
  if (building) { pending = true; return; }
  building = true;
  builtFingerprint = inputsFingerprint();
  eventSinceBuild = false;
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
  eventSinceBuild = true;
  if (timer) clearTimeout(timer);
  timer = setTimeout(runBuild, DEBOUNCE_MS);
}

const watchers = [];

function startWatching() {
  for (const dir of watchDirs) {
    try {
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

function stopWatching() {
  for (const w of watchers) { try { w.close(); } catch {  } }
  watchers.length = 0;
}

let probeFailing = false;
function pollInputs() {
  const current = inputsFingerprint();
  if (current === null) {
    if (!probeFailing) log('cannot read the committed inputs from git, so a deaf watch would go unnoticed');
    probeFailing = true;
    return;
  }
  probeFailing = false;
  if (builtFingerprint === null) {
    builtFingerprint = current;
    return;
  }
  if (pollAction({ built: builtFingerprint, current, eventSinceBuild }) === 'none') return;
  log('committed inputs changed with no file event since the last build: re-creating the watch and building');
  stopWatching();
  startWatching();
  schedule();
}

function shutdown() {
  stopWatching();
  if (child) child.kill('SIGTERM');
  process.exit(0);
}

const invokedDirectly = process.argv[1]
  && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (invokedDirectly) {
  runBuild();
  startWatching();
  setInterval(pollInputs, POLL_MS).unref();
  process.on('SIGTERM', shutdown);
  process.on('SIGINT', shutdown);
}
