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

import { spawn } from 'node:child_process';
import { watch } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const APP_DIR = dirname(fileURLToPath(import.meta.url));
const DEBOUNCE_MS = 200; // coalesce git-merge bursts (Apply touches many files at once)

// A clean build re-reads everything, so we only need to know "did anything
// change", not what. Watch the bundle's inputs: the app source tree, public/
// (sw.js / manifest / icons), the two root files that feed the build, and the
// SDK source (aliased into the bundle as @lucidos/sdk).
const watchDirs = [
  resolve(APP_DIR, 'src'),
  resolve(APP_DIR, 'public'),
  resolve(APP_DIR, '../../packages/lucidos-sdk/src'),
];
const watchFiles = [
  resolve(APP_DIR, 'index.html'),
  resolve(APP_DIR, 'vite.config.ts'),
];

let building = false;
let pending = false;
let child = null;

function runBuild() {
  if (building) { pending = true; return; }
  building = true;
  const started = Date.now();
  // Fresh child process every time → no incremental cache to wedge.
  // LUCIDOS_ATOMIC_DIST makes the build stage into dist.staging and atomically
  // publish onto dist/ only on success, so a failed build never clobbers dist/.
  child = spawn('npx', ['vite', 'build'], {
    cwd: APP_DIR,
    env: { ...process.env, LUCIDOS_ATOMIC_DIST: '1' },
    stdio: 'inherit',
  });
  child.on('exit', (code) => {
    child = null;
    building = false;
    const ms = Date.now() - started;
    console.log(`[dev-build-watch] vite build ${code === 0 ? 'ok' : `FAILED (exit ${code})`} in ${ms}ms`);
    if (pending) { pending = false; runBuild(); }
  });
}

let timer = null;
function schedule() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(runBuild, DEBOUNCE_MS);
}

// Initial build — workspace.sh waits for dist/index.html before starting the engine.
runBuild();

const watchers = [];
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

function shutdown() {
  for (const w of watchers) { try { w.close(); } catch { /* already closed */ } }
  if (child) child.kill('SIGTERM');
  process.exit(0);
}
process.on('SIGTERM', shutdown);
process.on('SIGINT', shutdown);
