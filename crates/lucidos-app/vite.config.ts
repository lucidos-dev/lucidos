/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';
import { resolve } from 'path';
import fs from 'fs';
import crypto from 'crypto';
import { normalizeCss, isCssWedge, type CssBuildSnapshot } from './src/dev/cssWedgeDetect';

const VITE_PORT = parseInt(process.env.VITE_PORT || '5173');

// Resolve TLS cert/key: local .certs/ first, then LUCIDOS_TLS_CERT/KEY env vars
// (needed in worktrees where .certs/ is gitignored — mirrors detect_tls() in workspace.sh).
function resolveTlsFile(localName: string, envVar: string | undefined): string | undefined {
  const localPath = resolve(__dirname, '../../.certs', localName);
  if (fs.existsSync(localPath)) return localPath;
  if (envVar && fs.existsSync(envVar)) return envVar;
  return undefined;
}
const certFile = resolveTlsFile('cert.pem', process.env.LUCIDOS_TLS_CERT);
const keyFile = resolveTlsFile('key.pem', process.env.LUCIDOS_TLS_KEY);
const hasCerts = !!(certFile && keyFile);

/**
 * Bake the engine's CalVer VERSION into the client bundle as the virtual module
 * `virtual:engine-version`. The engine and client ship together — comparing the
 * bundled value against the running engine's reported version surfaces drift
 * after engine-only restarts (engine new, loaded JS old → user needs refresh)
 * and after Vite rebuilds (client new, engine old → user needs restart).
 *
 * Re-reads on each module load and invalidates when VERSION changes, so the
 * engine-only restart path (which doesn't restart Vite) still picks up bumps —
 * in BOTH run modes: `addWatchFile` in `load` registers VERSION as a build
 * dependency so `vite build --watch` (the `--built` dev mode) re-runs `load`
 * and re-bakes ENGINE_VERSION on a bump, and `configureServer` does the same
 * for the live `--hmr` dev server. Without the `addWatchFile`, Rollup's watch
 * treated the virtual module as dependency-free and cached it forever, freezing
 * the bundled Client version at whatever VERSION was when the watch started —
 * so the control panel showed a permanent "Client (latest: …)" that no refresh
 * could clear, since every rebuilt bundle still carried the stale value.
 */
function engineVersionPlugin(): Plugin {
  const engineVersionFile = resolve(__dirname, '../lucidos-engine/VERSION');
  const moduleId = 'virtual:engine-version';
  const resolvedId = '\0' + moduleId;
  function readVersion(): string {
    try {
      return fs.readFileSync(engineVersionFile, 'utf-8').trim();
    } catch {
      return '0.0.0-dev';
    }
  }
  return {
    name: 'engine-version',
    resolveId(id) {
      if (id === moduleId) return resolvedId;
    },
    load(id) {
      if (id === resolvedId) {
        // Declare the dependency so `vite build --watch` re-runs this load (and
        // rebuilds the bundle → new asset hashes → new sw.js BUILD_ID → update
        // toast) whenever VERSION changes on disk.
        this.addWatchFile(engineVersionFile);
        return `export const ENGINE_VERSION = ${JSON.stringify(readVersion())};`;
      }
    },
    configureServer(server) {
      server.watcher.add(engineVersionFile);
      server.watcher.on('change', (path) => {
        if (path === engineVersionFile) {
          const mod = server.moduleGraph.getModuleById(resolvedId);
          if (mod) server.moduleGraph.invalidateModule(mod);
        }
      });
    },
  };
}

/**
 * Suppress Vite full-reload during git merge bursts.
 *
 * When changes are applied (git merge), many files update at once. Vite detects
 * these and falls back to a full page reload. This plugin detects the burst
 * pattern (≥3 files within 300ms) and suppresses full-reload for a short window,
 * letting CSS hot-update silently and deferring JS changes to the next manual
 * refresh. A custom HMR event notifies the client so it can show an "update
 * available" indicator.
 */
function suppressMergeReload(): Plugin {
  const timestamps: number[] = [];
  const BURST_WINDOW = 300;      // ms — detect burst if ≥3 changes within this
  const BURST_THRESHOLD = 3;
  const SUPPRESS_WINDOW = 5000;  // ms — suppress full-reload for this long after burst
  let suppressUntil = 0;

  return {
    name: 'suppress-merge-reload',
    handleHotUpdate({ file, server }) {
      const now = Date.now();
      // Only count non-CSS files for burst detection — CSS hot-updates via
      // stylesheet replacement and never triggers full-reload on its own.
      if (!file.endsWith('.css')) {
        timestamps.push(now);
        while (timestamps.length > 0 && now - timestamps[0] > BURST_WINDOW) {
          timestamps.shift();
        }
        if (timestamps.length >= BURST_THRESHOLD) {
          suppressUntil = now + SUPPRESS_WINDOW;
          timestamps.length = 0;
        }
      }

      if (now < suppressUntil) {
        if (file.endsWith('.css')) return undefined;
        server.ws.send({ type: 'custom', event: 'lucidos:update-available' });
        return [];
      }
      return undefined;
    },
  };
}

/**
 * Expose the per-build id to the app bundle as the virtual module
 * `virtual:build-id` (`CLIENT_BUILD_ID`). The module ships the
 * `__LUCIDOS_BUILD_ID__` placeholder; the `lucidos-sw-stamp` plugin's
 * writeBundle rewrites it to the real id in the emitted JS — the SAME id it
 * stamps into sw.js — so the running bundle knows EXACTLY which build produced
 * it.
 *
 * This is what lets the client judge "is the loaded code stale?" honestly: it
 * compares CLIENT_BUILD_ID (the build that produced the code executing now)
 * against the served sw.js BUILD_ID, instead of the controlling service
 * worker's id (which can run ahead of the loaded page after a claim-without-
 * reload). No `apply` gate — the virtual module must resolve in both `vite
 * serve` (the live dev server) and `vite build`; in serve the stamp plugin is
 * inert, so it stays the literal `__…__` placeholder, which
 * syncClientUpdateFromBuild treats as "no signal" (no update).
 */
function buildIdVirtualModule(): Plugin {
  const moduleId = 'virtual:build-id';
  const resolvedId = '\0' + moduleId;
  return {
    name: 'lucidos-build-id',
    resolveId(id) {
      if (id === moduleId) return resolvedId;
    },
    load(id) {
      if (id === resolvedId) {
        return `export const CLIENT_BUILD_ID = '__LUCIDOS_BUILD_ID__';`;
      }
    },
  };
}

/**
 * Stamp each production build (the `--built` dev mode and the shipped app) with a
 * per-build id, replacing the `__LUCIDOS_BUILD_ID__` placeholder in two places:
 *
 *  - dist/sw.js — folded into the cache name, so every rebuild is a
 *    byte-different sw.js. That byte difference is what the browser's
 *    service-worker update check detects, firing the client's "New version
 *    available → Refresh" toast (hooks/useStartup.ts). Without it a rebuild
 *    produces an identical sw.js and the toast never fires.
 *  - the emitted app JS — the `virtual:build-id` module's `CLIENT_BUILD_ID`, so
 *    the running code carries its own build id and can compare it against the
 *    served sw.js to detect when the loaded bundle is stale.
 *
 * The id is derived from the emitted asset filenames (which embed content
 * hashes), so it is DETERMINISTIC: a no-op rebuild (e.g. relaunching the dev
 * harness with unchanged source) yields the same id and does not spuriously
 * report an update. Rewriting the placeholder in already-hashed JS leaves the
 * filename (and thus the next build's id) untouched, so determinism holds. Vite
 * copies public/ (including sw.js) into dist/ at BUNDLE_START — before this
 * writeBundle hook — so dist/sw.js already exists here. `apply: 'build'` keeps
 * it inert during `vite serve`, where the literal placeholder is harmless.
 */
function stampServiceWorker(): Plugin {
  return {
    name: 'lucidos-sw-stamp',
    apply: 'build',
    writeBundle(options, bundle) {
      const outDir = options.dir ?? resolve(__dirname, 'dist');
      const swPath = resolve(outDir, 'sw.js');
      if (!fs.existsSync(swPath)) return;
      const assetNames = Object.keys(bundle).sort().join('\n');
      const buildId = crypto.createHash('sha256').update(assetNames).digest('hex').slice(0, 12);
      const stamp = (filePath: string): void => {
        if (!fs.existsSync(filePath)) return;
        const src = fs.readFileSync(filePath, 'utf-8');
        if (!src.includes('__LUCIDOS_BUILD_ID__')) return;
        fs.writeFileSync(filePath, src.replace(/__LUCIDOS_BUILD_ID__/g, buildId));
      };
      // sw.js (cache name + update-detect byte change) is copied from public/,
      // so it's not a bundle key — stamp it by path. The app JS chunks carrying
      // CLIENT_BUILD_ID are bundle keys — stamp each one.
      stamp(swPath);
      for (const name of Object.keys(bundle)) {
        if (name.endsWith('.js')) stamp(resolve(outDir, name));
      }
    },
  };
}

/**
 * Atomic dist publish for `vite build --watch` (the `--built` dev mode).
 *
 * Vite empties the outDir at the start of every (re)build, so an interrupted or
 * failed watch rebuild leaves the SERVED dist/ with only the public/ copy and no
 * index.html — `vite preview` then 404s every route until the next *successful*
 * rebuild, which only fires on the next source change. That exact failure took
 * the dev frontend down. To make a failed build a no-op for the running app,
 * this plugin (active only when LUCIDOS_ATOMIC_DIST is set — i.e. the built-watch
 * launch in workspace.sh) redirects the build to a staging dir and atomically
 * publishes it onto the live dist/ in `closeBundle`, which Rollup runs ONLY after
 * a complete build (and after every `writeBundle`, so the sw-stamp has already
 * run on the staged files). A crashed build never reaches closeBundle, so the
 * last good dist/ stays in place and preview keeps serving it.
 *
 * Production builds (npm run build / CI / Tauri) run without the env var → outDir
 * stays the default dist/, no staging, byte-identical to before.
 */
function atomicDistPublish(): Plugin {
  const enabled = !!process.env.LUCIDOS_ATOMIC_DIST;
  const staging = resolve(__dirname, 'dist.staging');
  const live = resolve(__dirname, 'dist');
  const prev = resolve(__dirname, 'dist.prev');
  return {
    name: 'lucidos-atomic-dist-publish',
    apply: 'build',
    config() {
      if (enabled) return { build: { outDir: 'dist.staging' } };
    },
    closeBundle() {
      if (!enabled) return;
      // Refuse to publish a build that didn't emit the app shell — never let a
      // degenerate build clobber a working dist/.
      if (!fs.existsSync(resolve(staging, 'index.html'))) {
        console.warn('[atomic-dist] staged build has no index.html — keeping previous dist/');
        return;
      }
      // rename() is atomic but can't overwrite a populated dir, so swap via a
      // backup: the only window where dist/ is absent is the two back-to-back
      // renames (sub-millisecond), and only on a SUCCESSFUL build.
      fs.rmSync(prev, { recursive: true, force: true });
      if (fs.existsSync(live)) fs.renameSync(live, prev);
      fs.renameSync(staging, live);
      fs.rmSync(prev, { recursive: true, force: true });
    },
  };
}

/**
 * Warn when the long-running `vite build --watch` (the `--built` dev mode) has
 * wedged its CSS pipeline and is serving STALE CSS.
 *
 * The watch survives engine-only Apply restarts (workspace.sh keeps it running),
 * so a single watch process can stay alive for days across many rebuilds. Once
 * observed (after a 1.5-day-old watch + a large class rename), it kept re-emitting
 * fresh JS from changed source while serving a FROZEN, stale CSS bundle — the
 * served index.html paired new JS (new class names) with old CSS (old class
 * names), so every renamed/new class had no rule and the app rendered unstyled,
 * SILENTLY. The fix is operational (restart the frontend → clean build), but the
 * desync was invisible until the bundles were diffed by hand.
 *
 * This guard makes it loud. Its signature is precise: read the CSS *source* fresh
 * from disk each build (independent of Vite's wedged module cache) and compare a
 * normalized hash of it against the previous build; if the source changed but the
 * emitted CSS bundle did NOT, the pipeline is serving stale CSS — log it. The
 * `lucidos:css-staleness` line lands in `<ws>/.lucidos/build-watch.log` the
 * instant the wedge produces a desync, turning an hour of bundle archaeology into
 * one log line.
 *
 * Scoped to the dev build-watch (LUCIDOS_ATOMIC_DIST) — a single-shot production
 * build (`npm run build` / CI / Tauri) is a fresh process and cannot wedge, and
 * stays byte-identical. Warn-only and fully try/catch-guarded: a guard must never
 * break a build or alter its output.
 */
function cssStalenessGuard(): Plugin {
  const enabled = !!process.env.LUCIDOS_ATOMIC_DIST;
  const srcDir = resolve(__dirname, 'src');
  let prev: CssBuildSnapshot | null = null;

  function collectCss(dir: string, out: string[]): void {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const p = resolve(dir, entry.name);
      if (entry.isDirectory()) collectCss(p, out);
      else if (entry.name.endsWith('.css')) out.push(p);
    }
  }

  return {
    name: 'lucidos-css-staleness-guard',
    apply: 'build',
    writeBundle(_options, bundle) {
      if (!enabled) return;
      try {
        const files: string[] = [];
        collectCss(srcDir, files);
        files.sort();
        const normalized = files
          .map((f) => normalizeCss(fs.readFileSync(f, 'utf-8')))
          .join('\n');
        const curr: CssBuildSnapshot = {
          cssSourceHash: crypto.createHash('sha256').update(normalized).digest('hex').slice(0, 16),
          cssOutputFingerprint: Object.keys(bundle).filter((k) => k.endsWith('.css')).sort().join('\n'),
        };
        if (isCssWedge(prev, curr)) {
          console.warn(
            '\n[lucidos:css-staleness] CSS source changed but the emitted CSS bundle did NOT — ' +
            'the long-running `vite build --watch` has wedged its CSS cache and is serving STALE styles ' +
            '(fresh JS, frozen CSS → renamed/new classes have no rules and render unstyled). ' +
            'Restart the frontend build: `./scripts/stop.sh -w <ws> && ./scripts/web-dev.sh -w <ws>`.\n'
          );
        }
        prev = curr;
      } catch {
        // A guard must never break the build — skip silently on any error.
      }
    },
  };
}

export default defineConfig({
  plugins: [engineVersionPlugin(), buildIdVirtualModule(), suppressMergeReload(), stampServiceWorker(), cssStalenessGuard(), preact(), atomicDistPublish()],
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          highlight: ['highlight.js'],
          marked: ['marked'],
        },
      },
    },
  },
  test: {
    // Cover both .test.ts and .test.tsx — JSX-bearing tests live in .tsx.
    // Without the explicit `.tsx` glob, files like
    // `src/components/chat/__tests__/permission-card.test.tsx` silently never
    // run (no error, just zero discovery — a vitest convention quirk).
    include: [
      'src/**/*.test.ts',
      'src/**/*.test.tsx',
      '../../packages/lucidos-sdk/src/**/*.test.ts',
    ],
    setupFiles: ['src/test-setup.ts'],
  },
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
      '@lucidos/sdk': resolve(__dirname, '../../packages/lucidos-sdk/src/index.ts'),
    },
  },
  server: {
    host: true,
    port: VITE_PORT,
    strictPort: true,
    hmr: {
      // HMR WebSocket connects directly to Vite (not through engine proxy).
      // The browser opens the engine port, which reverse-proxies HTTP to Vite,
      // but WebSocket HMR needs a direct connection to Vite's own port.
      port: VITE_PORT,
      protocol: hasCerts ? 'wss' : 'ws',
    },
    ...(hasCerts && {
      https: {
        cert: fs.readFileSync(certFile!),
        key: fs.readFileSync(keyFile!),
      },
    }),
    // No proxy needed — browser opens engine port directly, engine reverse-proxies
    // unmatched requests to Vite via LUCIDOS_DEV_PROXY in dev mode.
  },
  // `vite preview` (used by web-dev.sh --built to serve the built dist/) reads
  // `preview.*`, NOT `server.*` — mirror host/port/strictPort and TLS here so the
  // engine proxy and the iPhone reach it identically over Tailscale.
  preview: {
    host: true,
    port: VITE_PORT,
    strictPort: true,
    ...(hasCerts && {
      https: {
        cert: fs.readFileSync(certFile!),
        key: fs.readFileSync(keyFile!),
      },
    }),
  },
});
