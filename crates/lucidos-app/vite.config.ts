/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';
import { resolve } from 'path';
import fs from 'fs';
import crypto from 'crypto';
import {
  previewAuthGate,
  previewBindHost,
  previewGatewayFromEnv,
  previewProxy,
  previewTlsFromEnv,
} from './vite/frontendPreviewGateway';
import { askGatewaySession, loopbackAgent } from './vite/gatewaySession';
import { previewHttpsOptions } from './vite/previewTls';
import { chunkBudget, underDevBuildWatch } from './vite/chunkBudget';
import { smallLazyChunk, smallLazyChunks } from './vite/smallLazyChunks';
import { shellChunkPreload } from './vite/shellChunkPreload';
import { inlineBootScript } from './vite/inlineBootScript';
import { layoutMediaPlugin } from './vite/layoutMediaPlugin';
import { readHeadCommit, sourceCommitStamp } from './vite/sourceCommitStamp';
import { installedClientInventory } from './vite/installedClientInventory';
import { ensureGenerated } from './vite/ensureGenerated';

ensureGenerated(resolve(__dirname, '../..'));

const VITE_PORT = parseInt(process.env.VITE_PORT || '5173');

const previewGateway = previewGatewayFromEnv(process.env);
const previewAgent = previewGateway && loopbackAgent(previewGateway);

const previewTls = previewGateway && previewTlsFromEnv(process.env);
if (previewGateway && !previewTls) {
  throw new Error('The frontend preview needs its certificate, and the engine named none.');
}

function resolveTlsFile(localName: string, envVar: string | undefined): string | undefined {
  const localPath = resolve(__dirname, '../../.certs', localName);
  if (fs.existsSync(localPath)) return localPath;
  if (envVar && fs.existsSync(envVar)) return envVar;
  return undefined;
}
const certFile = resolveTlsFile('cert.pem', process.env.LUCIDOS_TLS_CERT);
const keyFile = resolveTlsFile('key.pem', process.env.LUCIDOS_TLS_KEY);

const serving = previewTls
  ? { host: previewBindHost(previewTls), https: previewHttpsOptions(previewTls) }
  : {
      host: true as const,
      ...(certFile && keyFile && {
        https: { cert: fs.readFileSync(certFile), key: fs.readFileSync(keyFile) },
      }),
    };

function suppressMergeReload(): Plugin {
  const timestamps: number[] = [];
  const BURST_WINDOW = 300;
  const BURST_THRESHOLD = 3;
  const SUPPRESS_WINDOW = 5000;
  let suppressUntil = 0;

  return {
    name: 'suppress-merge-reload',
    handleHotUpdate({ file, server }) {
      const now = Date.now();
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

const BUILD_ID_PLACEHOLDER = '__LUCIDOS_BUILD_ID__';

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
        return `export const CLIENT_BUILD_ID = '${BUILD_ID_PLACEHOLDER}';`;
      }
    },
  };
}

function stampServiceWorker(): Plugin {
  return {
    name: 'lucidos-sw-stamp',
    apply: 'build',
    writeBundle(options, bundle) {
      const outDir = options.dir ?? resolve(__dirname, 'dist');
      const swPath = resolve(outDir, 'sw.js');
      if (!fs.existsSync(swPath)) return;
      const assetNames = Object.keys(bundle).sort().join('\n');
      const idLength = process.env.LUCIDOS_E2E_COVERAGE ? BUILD_ID_PLACEHOLDER.length : 12;
      const buildId = crypto.createHash('sha256').update(assetNames).digest('hex').slice(0, idLength);
      const stamp = (filePath: string): void => {
        if (!fs.existsSync(filePath)) return;
        const src = fs.readFileSync(filePath, 'utf-8');
        if (!src.includes(BUILD_ID_PLACEHOLDER)) return;
        fs.writeFileSync(filePath, src.replaceAll(BUILD_ID_PLACEHOLDER, buildId));
      };
      stamp(swPath);
      for (const name of Object.keys(bundle)) {
        if (name.endsWith('.js') || name.endsWith('.html')) stamp(resolve(outDir, name));
      }
    },
  };
}

const APPEARANCE_BOOT_MARKER = '<!-- lucidos:appearance-boot -->';

function inlineAppearanceBoot(): Plugin {
  const bundlePath = resolve(
    __dirname, '../../packages/lucidos-sdk/dist/appearance-boot.host.js',
  );
  return {
    name: 'lucidos-appearance-boot',
    transformIndexHtml(html) {
      if (!html.includes(APPEARANCE_BOOT_MARKER)) {
        throw new Error(
          `index.html is missing ${APPEARANCE_BOOT_MARKER}, so the appearance boot `
          + 'script has nowhere to go.',
        );
      }
      if (!fs.existsSync(bundlePath)) {
        throw new Error(
          `${bundlePath} is missing. Run: ./scripts/codegen.sh`,
        );
      }
      return inlineBootScript(html, APPEARANCE_BOOT_MARKER, fs.readFileSync(bundlePath, 'utf-8'));
    },
  };
}

function syncPublicDir(): Plugin {
  const enabled = !!process.env.LUCIDOS_ATOMIC_DIST;
  const publicDir = resolve(__dirname, 'public');
  return {
    name: 'lucidos-sync-public-dir',
    apply: 'build',
    writeBundle(options) {
      if (!enabled || !fs.existsSync(publicDir)) return;
      const outDir = options.dir ?? resolve(__dirname, 'dist');
      fs.cpSync(publicDir, outDir, { recursive: true });
    },
  };
}

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
      if (!fs.existsSync(resolve(staging, 'index.html'))) {
        console.warn('[atomic-dist] staged build has no index.html — keeping previous dist/');
        return;
      }
      fs.rmSync(prev, { recursive: true, force: true });
      if (fs.existsSync(live)) fs.renameSync(live, prev);
      fs.renameSync(staging, live);
      fs.rmSync(prev, { recursive: true, force: true });
    },
  };
}

export default defineConfig({
  base: './',
  plugins: [
    ...(previewGateway && previewAgent
      ? [previewAuthGate(previewGateway, (cookie) => askGatewaySession(previewGateway, previewAgent, cookie))]
      : []),
    buildIdVirtualModule(), suppressMergeReload(), inlineAppearanceBoot(), syncPublicDir(),
    stampServiceWorker(), preact(), smallLazyChunks(underDevBuildWatch(process.env, process.ppid)), chunkBudget(underDevBuildWatch(process.env, process.ppid)),
    shellChunkPreload(),
    sourceCommitStamp(!!process.env.LUCIDOS_ATOMIC_DIST, () => readHeadCommit(__dirname), resolve(__dirname, 'dist')),
    installedClientInventory(resolve(__dirname, 'dist'), resolve(__dirname, 'public'), () => {
      const constants = fs.readFileSync(resolve(__dirname, '../../packages/lucidos-sdk/src/generated/engine-constants.ts'), 'utf8');
      const match = /^export const WORKSPACE_CLIENT_VERSION = (\d+);$/m.exec(constants);
      if (!match) throw new Error('Missing generated workspace-client version');
      return Number(match[1]);
    }),
    atomicDistPublish(),
  ],
  css: { postcss: { plugins: [layoutMediaPlugin()] } },
  build: {
    chunkSizeWarningLimit: Infinity,
    target: 'es2022',
    sourcemap: process.env.LUCIDOS_E2E_COVERAGE ? 'hidden' : false,
    rollupOptions: {
      output: {
        onlyExplicitManualChunks: true,
        manualChunks(id) {
          const shell = smallLazyChunk(id);
          if (shell) return shell;
          if (id.endsWith('/src/utils/highlightLibrary.ts')) return 'highlight';
          if (id.endsWith('/src/utils/markdownLibraries.ts')) return 'marked';
          if (id.includes('commonjsHelpers')) return 'vendor';
          if (id.includes('node_modules')) {
            if (id.includes('highlight.js')) return 'highlight';
            if (id.includes('marked') || id.includes('dompurify')) return 'marked';
            if (id.includes('jsqr')) return 'jsqr';
            return 'vendor';
          }
        },
      },
    },
  },
  test: {
    include: [
      'src/**/*.test.ts',
      'src/**/*.test.tsx',
      '../../packages/lucidos-sdk/src/**/*.test.ts',
      'e2e/**/*.test.ts',
    ],
    setupFiles: ['src/test-setup.ts'],
    pool: 'threads',
  },
  resolve: {
    alias: {
      '@': resolve(__dirname, 'src'),
      '@lucidos/restore-generation': resolve(__dirname, '../../packages/lucidos-sdk/src/_restoreGeneration.ts'),
      '@lucidos/sdk': resolve(__dirname, '../../packages/lucidos-sdk/src/index.ts'),
      '@lucidos/appearance': resolve(__dirname, '../../packages/lucidos-sdk/src/appearance.ts'),
      '@lucidos/font-faces': resolve(__dirname, '../../packages/lucidos-sdk/src/fontFaces.ts'),
      '@lucidos/geometry': resolve(__dirname, '../../packages/lucidos-sdk/src/geometry.ts'),
      '@lucidos/motion': resolve(__dirname, 'src/utils/motion.ts'),
      '@lucidos/tooltip': resolve(__dirname, '../../packages/lucidos-sdk/src/tooltip.ts'),
      '@lucidos/event-stream': resolve(__dirname, '../../packages/lucidos-sdk/src/eventStream.ts'),
      '@lucidos/find': resolve(__dirname, '../../packages/lucidos-sdk/src/find.ts'),
      '@lucidos/text-entry': resolve(__dirname, '../../packages/lucidos-sdk/src/textEntry.ts'),
      '@lucidos/pull-to-refresh': resolve(__dirname, '../../packages/lucidos-sdk/src/pullToRefresh.ts'),
      '@lucidos/pane-swipe': resolve(__dirname, '../../packages/lucidos-sdk/src/paneSwipe.ts'),
      '@lucidos/tap-press': resolve(__dirname, '../../packages/lucidos-sdk/src/tapPress.ts'),
      '@lucidos/link-press-guard': resolve(__dirname, '../../packages/lucidos-sdk/src/linkPressGuard.ts'),
      '@lucidos/web-link': resolve(__dirname, '../../packages/lucidos-sdk/src/webLink.ts'),
      '@lucidos/app-scroll': resolve(__dirname, '../../packages/lucidos-sdk/src/appScroll.ts'),
      '@lucidos/context-menu': resolve(__dirname, '../../packages/lucidos-sdk/src/contextMenu.ts'),
      '@lucidos/widget-frame': resolve(__dirname, '../../packages/lucidos-sdk/src/widgetFrame.ts'),
      '@lucidos/haptic': resolve(__dirname, '../../packages/lucidos-sdk/src/haptic.ts'),
      '@lucidos/one-sound': resolve(__dirname, '../../packages/lucidos-sdk/src/oneSound.ts'),
      '@lucidos/preference-catalog': resolve(__dirname, '../../packages/lucidos-sdk/src/generated/preference-catalog.ts'),
      '@lucidos/engine-constants': resolve(__dirname, '../../packages/lucidos-sdk/src/generated/engine-constants.ts'),
    },
  },
  server: {
    ...serving,
    port: VITE_PORT,
    strictPort: true,
    hmr: {
      port: VITE_PORT,
      protocol: 'https' in serving ? 'wss' : 'ws',
    },
    ...(previewGateway && previewAgent && {
      proxy: previewProxy(previewGateway, previewAgent),
      allowedHosts: true,
    }),
  },
  preview: {
    ...serving,
    port: VITE_PORT,
    strictPort: true,
  },
});
