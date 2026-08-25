/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';
import { resolve } from 'path';
import fs from 'fs';
import crypto from 'crypto';
import { frontendPreviewProxy, PREVIEW_API_ORIGIN_ENV } from './vite/frontendPreviewProxy';

const VITE_PORT = parseInt(process.env.VITE_PORT || '5173');

// The frontend preview (engine/frontend_preview.rs) runs THIS dev server from a
// coding-agent worktree on its own port, and forwards the engine-owned prefixes
// back to the engine so the page is same-origin with its own API. `undefined`
// for every other invocation, including a manual `npm run dev`.
const previewProxy = frontendPreviewProxy(process.env[PREVIEW_API_ORIGIN_ENV]);

// Resolve TLS cert/key: local .certs/ first, then LUCIDOS_TLS_CERT/KEY env
// vars, which a worktree needs because .certs/ is gitignored there. Mirrors
// detect_tls() in workspace.sh.
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
 * `__LUCIDOS_BUILD_ID__` placeholder, and the `lucidos-sw-stamp` plugin's
 * writeBundle rewrites it to the real id in the emitted JS: the SAME id it
 * stamps into sw.js. This is the client's own identity, never the engine's
 * VERSION (ADR 0069).
 *
 * No `apply` gate: the virtual module must resolve in both `vite serve` and
 * `vite build`. In serve the stamp plugin is inert, so the literal placeholder
 * stands, which syncClientUpdateFromBuild treats as "no signal".
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
 * The id is derived from the emitted asset filenames, which embed content
 * hashes, so it is DETERMINISTIC: a no-op rebuild yields the same id and does
 * not spuriously report an update. Rewriting the placeholder in already-hashed
 * JS leaves the filename, and so the next build's id, untouched.
 *
 * sw.js must already be in the outDir here. A single-shot build copies public/
 * before this writeBundle hook, and the dev build-watch's `syncPublicDir`
 * (ordered before this plugin) re-copies it on every rebuild.
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
 * Inline the appearance FOUC script into `index.html`'s `<head>`.
 *
 * The script has to be parser-blocking and inline: it resolves the device's
 * theme, font, scale and style overrides onto `<html>` before any stylesheet is
 * parsed or any module loads, so the first frame is already the user's
 * appearance. A `<script src>` would cost a round trip before first paint, and
 * an import is not available to it at all.
 *
 * That is a runtime constraint, not a source one. The same program is served to
 * every app iframe by the engine, and until this plugin the two documents
 * carried two hand-copied copies of it. Both now come from
 * `packages/lucidos-sdk/src/boot/`, built by that package into a committed
 * bundle, and this plugin substitutes the shell's build into a marker comment.
 *
 * `transformIndexHtml` runs in `serve` as well as `build`, so the dev server
 * gets it too. Reading the bundle per transform (rather than once at config
 * time) is what makes an incremental dev rebuild pick up an edit.
 */
const APPEARANCE_BOOT_MARKER = '<!-- lucidos:appearance-boot -->';

function inlineAppearanceBoot(): Plugin {
  const bundlePath = resolve(
    __dirname, '../../packages/lucidos-sdk/src/generated/appearance-boot.host.js',
  );
  return {
    name: 'lucidos-appearance-boot',
    transformIndexHtml(html) {
      if (!html.includes(APPEARANCE_BOOT_MARKER)) {
        // Fail the build rather than serve a shell with no FOUC script: the
        // symptom would be a flash of the wrong theme on every cold load, which
        // reads as a styling bug and not as a missing marker.
        throw new Error(
          `index.html is missing ${APPEARANCE_BOOT_MARKER}, so the appearance boot `
          + 'script has nowhere to go.',
        );
      }
      if (!fs.existsSync(bundlePath)) {
        throw new Error(
          `${bundlePath} is missing. Run: cd packages/lucidos-sdk && npm run build`,
        );
      }
      const bundle = fs.readFileSync(bundlePath, 'utf-8');
      // An HTML parser ends a <script> at the first `</script`, inside a string
      // or a regex literal alike, so a bundle carrying that sequence would close
      // its own tag and spill the remainder into the document as markup. Nothing
      // in the boot source does today; this is here because inlining arbitrary
      // built text is exactly where that stops being true quietly.
      if (/<\/script/i.test(bundle)) {
        throw new Error(
          'The appearance boot bundle contains `</script`, which would terminate the '
          + 'inline tag early. Rewrite the source so the sequence cannot appear '
          + '(e.g. split the string).',
        );
      }
      return html.replace(APPEARANCE_BOOT_MARKER, `<script>\n${bundle}</script>`);
    },
  };
}

/**
 * Re-copy public/ into the build outDir on EVERY build of the dev build-watch.
 *
 * `vite build --watch` copies publicDir only on the INITIAL build. Public files
 * are not part of the bundle graph, so the watcher skips them on incremental
 * rebuilds (vitejs/vite#18655). Combined with `atomicDistPublish` swapping the
 * whole staging dir onto the live dist/, the first incremental rebuild WIPES
 * them from the served dist/. The engine then serves the SPA shell for /sw.js,
 * so service-worker registration fails on a MIME error and the PWA manifest and
 * icons 404.
 *
 * This runs in writeBundle ordered BEFORE stampServiceWorker, so the freshly
 * re-copied sw.js is present for its BUILD_ID stamp. Copying after the stamp
 * would overwrite it with the unstamped source. Scoped to the dev build-watch
 * (LUCIDOS_ATOMIC_DIST), since a single-shot production build copies public
 * correctly on its own.
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
 * Vite empties the outDir at the start of every rebuild. So a failed watch
 * rebuild leaves the SERVED dist/ with only the public/ copy and no
 * index.html. `vite preview` then 404s every route until the next successful
 * rebuild, which only fires on the next source change.
 *
 * To make a failed build a no-op for the running app, this plugin redirects the
 * build to a staging dir. It publishes atomically onto the live dist/ in
 * `closeBundle`, which Rollup runs ONLY after a complete build, and after every
 * `writeBundle`. A crashed build never reaches closeBundle, so the last good
 * dist/ stays in place. Active only under LUCIDOS_ATOMIC_DIST, so a production
 * build keeps the default dist/ with no staging.
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
      // Refuse to publish a build that did not emit the app shell: never let a
      // degenerate build clobber a working dist/.
      if (!fs.existsSync(resolve(staging, 'index.html'))) {
        console.warn('[atomic-dist] staged build has no index.html — keeping previous dist/');
        return;
      }
      // rename() is atomic but cannot overwrite a populated dir, so swap via a
      // backup. The only window where dist/ is absent is the two back-to-back
      // renames, and only on a SUCCESSFUL build.
      fs.rmSync(prev, { recursive: true, force: true });
      if (fs.existsSync(live)) fs.renameSync(live, prev);
      fs.renameSync(staging, live);
      fs.rmSync(prev, { recursive: true, force: true });
    },
  };
}

export default defineConfig({
  // Relative asset base (ADR 0013): the built index.html references its bundle
  // as `./assets/...`, so one build serves under both `/` and a workspace
  // behind the gateway, which injects a `<base href>` to scope them. At the
  // root with no `<base>` they resolve to `/assets/...`. In `vite serve` a
  // relative base falls back to `/`, so the dev server is unaffected.
  base: './',
  plugins: [buildIdVirtualModule(), suppressMergeReload(), inlineAppearanceBoot(), syncPublicDir(), stampServiceWorker(), preact(), atomicDistPublish()],
  build: {
    // The eager entry chunk is the first-paint-critical app core: shell, store,
    // event handling, signals, layout. Views are lazy-loaded and the heavy libs
    // are split out below, so the remaining core is irreducible without
    // lazy-loading first-paint code. Rollup's 500 kB default advisory is too
    // conservative for it.
    //
    // 600 is a CEILING, not a budget to spend. When it fires, code-split the
    // next thing the eager graph does not need on first paint, rather than
    // raising the number.
    chunkSizeWarningLimit: 600,
    rollupOptions: {
      output: {
        // Split third-party code out of the always-loaded entry chunk. The
        // markdown and highlight libs get their own buckets, being heavy and
        // pulled in only by the chat paths. Everything else under node_modules
        // lands in a shared `vendor` chunk. Without this the framework rides in
        // the entry chunk and pushes it past Rollup's advisory.
        manualChunks(id) {
          if (id.includes('node_modules')) {
            // Each name below is a dependency only ONE lazy chunk imports.
            // Without its own name the `vendor` catch-all takes it, and vendor
            // is loaded by the entry: a lazily-imported package then ships on
            // first paint anyway. `jsqr` is reached only by `PairingScanner`,
            // on the screen a cold PWA launch paints first.
            if (id.includes('highlight.js')) return 'highlight';
            // `dompurify` rides with `marked`: the sanitizer has exactly one
            // importer, and that importer is the markdown renderer.
            if (id.includes('marked') || id.includes('dompurify')) return 'marked';
            if (id.includes('jsqr')) return 'jsqr';
            return 'vendor';
          }
        },
      },
    },
  },
  test: {
    // Cover both .test.ts and .test.tsx, since JSX-bearing tests live in .tsx.
    // Without the explicit `.tsx` glob those files silently never run: no
    // error, just zero discovery.
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
      // The appearance boot contract, reached WITHOUT the SDK barrel above.
      // The barrel pulls the whole SDK in at module load. The host store
      // imports this from a module that installs an OS-theme listener at
      // import time, so widening that graph reorders side effects.
      // Mirrored in tsconfig.json `paths` so tsc resolves it too.
      '@lucidos/appearance': resolve(__dirname, '../../packages/lucidos-sdk/src/appearance.ts'),
      // The tooltip, and the viewport clamps it shares with the host's anchored
      // popover. Both are reached WITHOUT the barrel, for the same reason.
      // Mirrored in tsconfig.json `paths` so tsc resolves them too.
      '@lucidos/geometry': resolve(__dirname, '../../packages/lucidos-sdk/src/geometry.ts'),
      '@lucidos/tooltip': resolve(__dirname, '../../packages/lucidos-sdk/src/tooltip.ts'),
    },
  },
  server: {
    host: true,
    port: VITE_PORT,
    strictPort: true,
    hmr: {
      // HMR connects directly to Vite, not through the engine proxy. The
      // browser opens the engine port, which reverse-proxies HTTP to Vite, but
      // the WebSocket needs a direct connection to Vite's own port.
      port: VITE_PORT,
      protocol: hasCerts ? 'wss' : 'ws',
    },
    ...(hasCerts && {
      https: {
        cert: fs.readFileSync(certFile!),
        key: fs.readFileSync(keyFile!),
      },
    }),
    ...(previewProxy && { proxy: previewProxy }),
    // The `server` block serves a manual `vite serve` and the frontend preview
    // (engine/frontend_preview.rs). It is NOT part of the dev harness (ADR
    // 0014): web-dev, tauri-dev and e2e all build dist/ and let the engine
    // serve it via LUCIDOS_STATIC_DIR, with no engine-to-Vite proxy in the
    // workspace's own serving path. The preview is a separate origin the user
    // opens deliberately, never that path (ADR 0055).
  },
  // `vite preview` reads `preview.*`, NOT `server.*`. Mirror host, port,
  // strictPort and TLS here so the engine proxy and a phone reach it
  // identically.
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
