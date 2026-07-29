/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';
import { resolve } from 'path';
import fs from 'fs';
import crypto from 'crypto';

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
 * The engine's CalVer VERSION is deliberately NOT baked into the client bundle.
 *
 * A `virtual:engine-version` plugin used to do exactly that, so the System page
 * could show it as the "Client" version. It was a lie by construction: the value
 * froze at whatever VERSION happened to be when the bundle was last built, while
 * the running engine's own VERSION kept bumping on every engine-only Apply — two
 * numbers on one page that could disagree with nothing the user could do about it
 * (no reload changes a baked string). The `addWatchFile` that was supposed to
 * re-bake it went inert when the `--built` dev mode replaced `vite build --watch`
 * with `dev-build-watch.mjs`'s one-shot builds, which don't watch VERSION anyway.
 *
 * Keeping them in sync is worse than dropping the row: re-baking on every VERSION
 * bump makes each engine-only change produce a byte-different bundle → a new
 * sw.js BUILD_ID → a "New version available — refresh to sync" toast whose entire
 * payload is a version string. Today an engine-only Switch correctly surfaces
 * nothing (see store/actions/connection.ts). The client's honest identity is its
 * own `CLIENT_BUILD_ID` (`virtual:build-id`, below) — that is what the System
 * page shows and what the refresh badge compares.
 */

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
 * filename (and thus the next build's id) untouched, so determinism holds.
 * sw.js must already be in the outDir here: in a single-shot build Vite copies
 * public/ before this writeBundle hook, and in the dev build-watch
 * `syncPublicDir` (ordered before this plugin) re-copies it on every rebuild —
 * see that plugin for why Vite alone isn't enough. `apply: 'build'` keeps it
 * inert during `vite serve`, where the literal placeholder is harmless.
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
 * Re-copy public/ into the build outDir on EVERY build of the dev build-watch.
 *
 * `vite build --watch` copies publicDir only on the INITIAL build — public files
 * (sw.js, manifest.json, the favicons, icons/, splash/) aren't part of the
 * bundle graph, so the watcher skips them on incremental rebuilds
 * (vitejs/vite#18655). On its own that just leaves them stale; combined with
 * `atomicDistPublish` swapping the WHOLE staging dir onto the live dist/, the
 * first incremental rebuild WIPES them from the served dist/ entirely. The
 * engine then serves the SPA shell for /sw.js (text/html), so service-worker
 * registration fails with a MIME error ("push notifications may not work") and
 * the PWA manifest + icons 404 — on direct AND gateway access alike.
 *
 * This runs in writeBundle ordered BEFORE stampServiceWorker (earlier in the
 * plugins array) so the freshly re-copied sw.js is present for its BUILD_ID
 * stamp — copying after the stamp would overwrite it with the unstamped source
 * (IS_BUILT=false, no update toast). Scoped to the dev build-watch
 * (LUCIDOS_ATOMIC_DIST): a single-shot production build (npm run build / CI /
 * Tauri) copies public correctly on its own, so it stays byte-identical.
 */
function syncPublicDir(): Plugin {
  const enabled = !!process.env.LUCIDOS_ATOMIC_DIST;
  const publicDir = resolve(__dirname, 'public');
  return {
    name: 'lucidos-sync-public-dir',
    apply: 'build',
    writeBundle(options) {
      if (!enabled || !fs.existsSync(publicDir)) return;
      const outDir = options.dir ?? resolve(__dirname, 'dist');
      // Merge public/ children into outDir (overwriting); leaves the emitted
      // assets/ + index.html untouched (they aren't in public/).
      fs.cpSync(publicDir, outDir, { recursive: true });
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

export default defineConfig({
  // Relative asset base (ADR 0013): the built index.html references its bundle
  // as `./assets/…` so the same build serves under both `/` (dev/gateway root)
  // and `/ws/<id>/` (a workspace behind the gateway, where the gateway injects
  // `<base href="/ws/<id>/">` to scope these). At the root with no <base> they
  // resolve to `/assets/…` exactly as before — backward-compatible for the
  // engine static-serve, `vite preview`, and Tauri. In `vite serve` (HMR) a
  // relative base falls back to `/`, so the dev server is unaffected.
  base: './',
  plugins: [buildIdVirtualModule(), suppressMergeReload(), syncPublicDir(), stampServiceWorker(), preact(), atomicDistPublish()],
  build: {
    // The eager entry chunk is the first-paint-critical app core (shell, store,
    // SSE/event handling, signals, layout) — ~517 kB minified / ~154 kB gzipped,
    // a healthy first-load for a feature-rich SPA. Views are already extensively
    // lazy-loaded and the heavy libs (marked, highlight.js) + the framework
    // (vendor, below) are split out, so the remaining core is irreducible without
    // lazy-loading first-paint code (a UX regression) or gaming the per-chunk
    // metric. Rollup's 500 kB default advisory is too conservative here; raise it
    // to 600 kB so the guard still fires on a genuine regression (e.g. a heavy
    // lib accidentally pulled into the eager graph) without flagging the baseline.
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        // Split third-party code out of the always-loaded entry chunk. The
        // markdown/highlight libs get their own buckets (they're heavy and only
        // pulled in by the chat/markdown paths); everything else under
        // node_modules — preact, @preact/signals, @tauri-apps/api — lands in a
        // shared `vendor` chunk. Without this the framework rode in the entry
        // chunk, pushing it past Rollup's 500 kB advisory.
        manualChunks(id) {
          if (id.includes('node_modules')) {
            if (id.includes('highlight.js')) return 'highlight';
            if (id.includes('marked')) return 'marked';
            return 'vendor';
          }
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
    // The `server` block is only used by a manual `vite serve` (`npm run dev`).
    // It is NOT part of the dev harness anymore (ADR 0014): web-dev / tauri-dev /
    // e2e all build dist/ and the engine serves it directly via LUCIDOS_STATIC_DIR
    // — there is no engine→Vite proxy. Kept for standalone frontend iteration.
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
