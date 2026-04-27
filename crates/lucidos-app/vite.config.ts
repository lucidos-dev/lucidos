/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';
import { resolve } from 'path';
import fs from 'fs';

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
 * engine-only restart path (which doesn't restart Vite) still picks up bumps.
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

export default defineConfig({
  plugins: [engineVersionPlugin(), suppressMergeReload(), preact()],
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
    include: ['src/**/*.test.ts'],
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
});
