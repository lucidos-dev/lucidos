// @ts-expect-error: Node APIs exist where Vite runs, no @types/node in project
import fs from 'node:fs/promises';
// @ts-expect-error: same
import { resolve } from 'node:path';
import type { Plugin } from 'vite';
import { isWorkspaceSlug, parseWorkspaceRoute } from '../src/utils/basePath';

export function previewPathFromEnv(env: Record<string, string | undefined>): string | undefined {
  if (!env.LUCIDOS_FRONTEND_PREVIEW_SOCKET) return undefined;
  const slug = env.LUCIDOS_FRONTEND_PREVIEW_WORKSPACE_ID;
  const key = env.LUCIDOS_FRONTEND_PREVIEW_KEY;
  if (!slug || !isWorkspaceSlug(slug) || !key || !/^[a-f0-9]{16}$/.test(key)) {
    throw new Error('The preview needs a workspace slug and gateway preview key.');
  }
  return `/~preview/${key}/${slug}/`;
}

export function previewWorkspacePrefix(header: string | string[] | undefined, slug: string): string {
  if (typeof header === 'string' && header.startsWith('/') && header.endsWith('/')) {
    const route = parseWorkspaceRoute(header.slice(1, -1));
    if (route?.slug === slug) return header;
  }
  return `/${slug}/`;
}

export function stampPreviewBase(html: string, prefix: string): string {
  return html.replace(/<base\b[^>]*>/gi, '').replace(/<head\b[^>]*>/i, head => `${head}<meta name="lucidos-frontend-preview" content="true"><base href="${prefix}">`);
}

export function frontendPreviewPath(path: string, slug: string): Plugin {
  return {
    name: 'lucidos-frontend-preview-path',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const request = req as unknown as { url?: string; headers: Record<string, string | string[] | undefined> };
        const pathname = request.url?.split('?')[0];
        if (pathname !== path && pathname !== `${path}index.html`) return next();
        const prefix = previewWorkspacePrefix(request.headers['x-forwarded-prefix'], slug);
        void fs.readFile(resolve(server.config.root, 'index.html'), 'utf8')
          .then((html: string) => server.transformIndexHtml(request.url!, html))
          .then((html: string) => {
            res.setHeader('Content-Type', 'text/html; charset=utf-8');
            res.setHeader('Cache-Control', 'no-store');
            res.end(stampPreviewBase(html, prefix));
          }).catch(next);
      });
    },
  };
}
